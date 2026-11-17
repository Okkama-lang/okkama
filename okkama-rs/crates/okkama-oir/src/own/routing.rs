//! Exhaustive classification/rewrite routing. New opcodes must choose a path.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Handling {
    Scalar,
    PlainArray,
    Construction,
    Snapshot,
    Once,
    Dictionary,
    Call,
    Lexical,
    Elaborated,
    Async,
    Unsupported,
}

pub(super) fn handling(operation: &Operation) -> Handling {
    match operation {
        Operation::Const(_) => Handling::Scalar,
        Operation::Core(op) => match op {
            CoreOp::IntAdd { .. }
            | CoreOp::IntSub { .. }
            | CoreOp::IntMul { .. }
            | CoreOp::IntDiv { .. }
            | CoreOp::IntMod { .. }
            | CoreOp::IntAnd { .. }
            | CoreOp::IntOr { .. }
            | CoreOp::IntXor { .. }
            | CoreOp::IntShl { .. }
            | CoreOp::IntShr { .. }
            | CoreOp::IntUshr { .. }
            | CoreOp::IntNeg { .. }
            | CoreOp::IntNot { .. }
            | CoreOp::IntAbs { .. }
            | CoreOp::IntSign { .. }
            | CoreOp::IntBitCount { .. }
            | CoreOp::IntLeadingZeros { .. }
            | CoreOp::IntTrailingZeros { .. }
            | CoreOp::IntReverseBits { .. }
            | CoreOp::IntSwapBytes { .. }
            | CoreOp::IntRotateLeft { .. }
            | CoreOp::IntRotateRight { .. }
            | CoreOp::ByteAnd { .. }
            | CoreOp::ByteOr { .. }
            | CoreOp::ByteXor { .. }
            | CoreOp::ByteShl { .. }
            | CoreOp::ByteShr { .. }
            | CoreOp::ByteUshr { .. }
            | CoreOp::ByteNot { .. }
            | CoreOp::ByteBitCount { .. }
            | CoreOp::ByteLeadingZeros { .. }
            | CoreOp::ByteTrailingZeros { .. }
            | CoreOp::ByteReverseBits { .. }
            | CoreOp::ByteSwapBytes { .. }
            | CoreOp::ByteRotateLeft { .. }
            | CoreOp::ByteRotateRight { .. }
            | CoreOp::FloatAdd { .. }
            | CoreOp::FloatSub { .. }
            | CoreOp::FloatMul { .. }
            | CoreOp::FloatDiv { .. }
            | CoreOp::FloatMod { .. }
            | CoreOp::FloatPow { .. }
            | CoreOp::FloatNeg { .. }
            | CoreOp::FloatAbs { .. }
            | CoreOp::FloatSign { .. }
            | CoreOp::FloatSqrt { .. }
            | CoreOp::RatAdd { .. }
            | CoreOp::RatSub { .. }
            | CoreOp::RatMul { .. }
            | CoreOp::RatDiv { .. }
            | CoreOp::RatMod { .. }
            | CoreOp::RatNeg { .. }
            | CoreOp::RatAbs { .. }
            | CoreOp::RatSign { .. }
            | CoreOp::IntEq { .. }
            | CoreOp::IntNe { .. }
            | CoreOp::IntLt { .. }
            | CoreOp::IntLe { .. }
            | CoreOp::IntGt { .. }
            | CoreOp::IntGe { .. }
            | CoreOp::ByteEq { .. }
            | CoreOp::ByteNe { .. }
            | CoreOp::ByteLt { .. }
            | CoreOp::ByteLe { .. }
            | CoreOp::ByteGt { .. }
            | CoreOp::ByteGe { .. }
            | CoreOp::FloatEq { .. }
            | CoreOp::FloatNe { .. }
            | CoreOp::FloatLt { .. }
            | CoreOp::FloatLe { .. }
            | CoreOp::FloatGt { .. }
            | CoreOp::FloatGe { .. }
            | CoreOp::RatEq { .. }
            | CoreOp::RatNe { .. }
            | CoreOp::RatLt { .. }
            | CoreOp::RatLe { .. }
            | CoreOp::RatGt { .. }
            | CoreOp::RatGe { .. }
            | CoreOp::BoolEq { .. }
            | CoreOp::BoolNe { .. }
            | CoreOp::EnumEq { .. }
            | CoreOp::EnumNe { .. }
            | CoreOp::CharEq { .. }
            | CoreOp::CharNe { .. }
            | CoreOp::CharLt { .. }
            | CoreOp::CharLe { .. }
            | CoreOp::CharGt { .. }
            | CoreOp::CharGe { .. }
            | CoreOp::StrEq { .. }
            | CoreOp::StrNe { .. }
            | CoreOp::StrLt { .. }
            | CoreOp::StrLe { .. }
            | CoreOp::StrGt { .. }
            | CoreOp::StrGe { .. }
            | CoreOp::ValEq { .. }
            | CoreOp::ValNe { .. }
            | CoreOp::BoolNot { .. }
            | CoreOp::RatNew { .. }
            | CoreOp::RatNumerator { .. }
            | CoreOp::RatDenominator { .. }
            | CoreOp::Convert { .. }
            | CoreOp::ByteFrom { .. }
            | CoreOp::Undef { .. }
            | CoreOp::StrConcat { .. }
            | CoreOp::StrLen { .. }
            | CoreOp::StrUtf8 { .. }
            | CoreOp::StrSlice { .. }
            | CoreOp::StrSplit { .. }
            | CoreOp::StrReplace { .. }
            | CoreOp::StrReplaceFirst { .. }
            | CoreOp::StrRepeat { .. }
            | CoreOp::StrTrim { .. }
            | CoreOp::StrChomp { .. }
            | CoreOp::StrTrimStart { .. }
            | CoreOp::StrTrimEnd { .. }
            | CoreOp::StrLower { .. }
            | CoreOp::StrUpper { .. }
            | CoreOp::StrFind { .. }
            | CoreOp::StrFindLast { .. }
            | CoreOp::StrStartsWith { .. }
            | CoreOp::StrEndsWith { .. }
            | CoreOp::StrCharacters { .. }
            | CoreOp::RangeNew { .. }
            | CoreOp::RangeStart { .. }
            | CoreOp::RangeEnd { .. }
            | CoreOp::RangeLen { .. }
            | CoreOp::VariantTag { .. }
            | CoreOp::FailSecondary { .. }
            | CoreOp::FailCaused { .. }
            | CoreOp::FailIsThrown { .. }
            | CoreOp::TestOk { .. }
            | CoreOp::TestIs { .. }
            | CoreOp::TestIsnt { .. }
            | CoreOp::TestFail { .. }
            | CoreOp::EnumCase { .. } => Handling::Scalar,
            CoreOp::Global { .. }
            | CoreOp::VariantOf { .. }
            | CoreOp::Retag { .. }
            | CoreOp::VariantNew { .. }
            | CoreOp::VariantGet { .. }
            | CoreOp::FakeNew { .. } => Handling::Unsupported,
            CoreOp::ObjNew { .. }
            | CoreOp::ObjNext { .. }
            | CoreOp::ObjInit { .. }
            | CoreOp::ObjGet { .. }
            | CoreOp::ObjPending { .. }
            | CoreOp::SuccTake { .. } => Handling::Construction,
            CoreOp::OnceLoad { .. }
            | CoreOp::OnceStore { .. } => Handling::Once,
            CoreOp::RoleWrap { .. }
            | CoreOp::RoleNarrow { .. }
            | CoreOp::CapMake { .. }
            | CoreOp::FnClosure { .. }
            | CoreOp::FnBind { .. }
            | CoreOp::FnBindRole { .. }
            | CoreOp::AnyBox { .. } => Handling::Snapshot,
            CoreOp::TypeOf { .. } => Handling::Dictionary,
        },
        Operation::Call(op) => match op {
            CallOp::Direct { .. }
            | CallOp::Role { .. }
            | CallOp::Order { .. }
            | CallOp::Closure { .. }
            | CallOp::Cap { .. }
            | CallOp::Native { .. }
            | CallOp::Intrinsic { .. } => Handling::Call,
            CallOp::Resume { .. } => Handling::Async,
        },
        Operation::Collection(op) => match op {
            CollectionOp::ArrayNew { .. }
            | CollectionOp::ArrayLen { .. }
            | CollectionOp::ArrayGet { .. } => Handling::PlainArray,
            CollectionOp::ArraySlice { .. }
            | CollectionOp::ArrayConcat { .. }
            | CollectionOp::ArrayReplace { .. }
            | CollectionOp::ArrayRemove { .. }
            | CollectionOp::ArrayDifference { .. }
            | CollectionOp::MapNew { .. }
            | CollectionOp::MapMerge { .. }
            | CollectionOp::MapDifference { .. }
            | CollectionOp::ArrayJoin { .. }
            | CollectionOp::ArrayOrder { .. }
            | CollectionOp::ArrayRetag { .. }
            | CollectionOp::MapLen { .. }
            | CollectionOp::MapHas { .. }
            | CollectionOp::MapGet { .. }
            | CollectionOp::MapKeys { .. }
            | CollectionOp::MapValues { .. }
            | CollectionOp::MapKeyAt { .. }
            | CollectionOp::MapValueAt { .. }
            | CollectionOp::MapRetag { .. }
            | CollectionOp::BuilderNew { .. }
            | CollectionOp::BuilderPush { .. }
            | CollectionOp::BuilderAppend { .. }
            | CollectionOp::BuilderAppendRange { .. }
            | CollectionOp::BuilderFinish { .. }
            | CollectionOp::BuilderAbandon { .. }
            | CollectionOp::MapBuilderNew { .. }
            | CollectionOp::MapBuilderPut { .. }
            | CollectionOp::MapBuilderAppend { .. }
            | CollectionOp::MapBuilderFinish { .. }
            | CollectionOp::MapBuilderAbandon { .. } => Handling::Unsupported,
        },
        Operation::Carrier(op) => match op {
            CarrierOp::Attach { .. }
            | CarrierOp::Transfer { .. }
            | CarrierOp::Rehome { .. }
            | CarrierOp::Release { .. } => Handling::Unsupported,
        },
        Operation::Exit(op) => match op {
            ExitOp::Prepare { .. } => Handling::Lexical,
            ExitOp::Replace { .. }
            | ExitOp::Commit { .. }
            | ExitOp::Extend { .. }
            | ExitOp::Abort { .. } => Handling::Elaborated,
        },
        Operation::Cleanup(op) => match op {
            CleanupOp::Begin { .. }
            | CleanupOp::SettleEmpty { .. }
            | CleanupOp::Defer { .. }
            | CleanupOp::Settle { .. }
            | CleanupOp::SettleDynamic { .. }
            | CleanupOp::Plan { .. }
            | CleanupOp::Destroy { .. }
            | CleanupOp::DestroyDirect { .. }
            | CleanupOp::RecordFailure { .. }
            | CleanupOp::DeliverCancel { .. }
            | CleanupOp::Finish { .. }
            | CleanupOp::CellNew { .. }
            | CleanupOp::CellLoad { .. }
            | CleanupOp::CellStore { .. } => Handling::Elaborated,
        },
        Operation::Ownership(op) => match op {
            OwnershipOp::Dependencies { .. }
            | OwnershipOp::DepsJoin { .. }
            | OwnershipOp::Empty { .. }
            | OwnershipOp::Move { .. }
            | OwnershipOp::Join { .. }
            | OwnershipOp::Admit { .. }
            | OwnershipOp::Accept { .. }
            | OwnershipOp::Bind { .. }
            | OwnershipOp::Rebind { .. }
            | OwnershipOp::Stage { .. }
            | OwnershipOp::Split { .. }
            | OwnershipOp::SplitChecked { .. }
            | OwnershipOp::ScopeBegin { .. }
            | OwnershipOp::ScopeSplit { .. }
            | OwnershipOp::ScopeTransfer { .. }
            | OwnershipOp::Dynamic { .. }
            | OwnershipOp::DynamicCheck { .. }
            | OwnershipOp::DynamicRefresh { .. }
            | OwnershipOp::ScopeEnd { .. }
            | OwnershipOp::ScopeUnwind { .. }
            | OwnershipOp::End { .. } => Handling::Elaborated,
        },
        Operation::Async(op) => match op {
            AsyncOp::Await { .. }
            | AsyncOp::AwaitShielded { .. }
            | AsyncOp::AwaitTake { .. }
            | AsyncOp::UniqueCacheProof { .. }
            | AsyncOp::CacheRehome { .. }
            | AsyncOp::FutureSpawn { .. }
            | AsyncOp::FutureSpawnBound { .. }
            | AsyncOp::FutureCancel { .. }
            | AsyncOp::FutureDone { .. }
            | AsyncOp::FutureCancelled { .. }
            | AsyncOp::FutureNew { .. }
            | AsyncOp::DutyEmpty { .. }
            | AsyncOp::FuturePublish { .. }
            | AsyncOp::AwaitUnwind { .. }
            | AsyncOp::ServiceMaterialize { .. }
            | AsyncOp::ServiceAbsorb { .. }
            | AsyncOp::FutureSettle { .. }
            | AsyncOp::FrameAlloc { .. }
            | AsyncOp::FrameLoad { .. }
            | AsyncOp::FrameStore { .. }
            | AsyncOp::FrameSave { .. }
            | AsyncOp::FrameResume { .. }
            | AsyncOp::FrameCancel { .. }
            | AsyncOp::Pin { .. }
            | AsyncOp::PinTransfer { .. }
            | AsyncOp::Unpin { .. }
            | AsyncOp::WorkerBind { .. }
            | AsyncOp::WorkerSpawn { .. }
            | AsyncOp::ProcessSpawn { .. } => Handling::Async,
        },
        Operation::Lexical(op) => match op {
            LexicalOp::Bind { .. }
            | LexicalOp::Rebind { .. }
            | LexicalOp::Stage { .. }
            | LexicalOp::Temp { .. }
            | LexicalOp::DeferRegister { .. }
            | LexicalOp::Want { .. }
            | LexicalOp::Discard { .. } => Handling::Lexical,
        },
    }
}
