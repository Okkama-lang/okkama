//! Collection and variant helpers for the synchronous rewrite (spec §4.1, §4.2).
//!
//! High collections name no element packets. Derived collections and variants
//! leave inserted obligations with their owners and retain them through full
//! deps; builders thread an explicit element journal beside the linear buffer.
use super::*;

/// The element packet a collection operation consumes, if any.
pub(super) fn packet(operation: &CollectionOp) -> Option<ValueId> {
    match operation {
        CollectionOp::ArrayNew { packet, .. } | CollectionOp::ArraySlice { packet, .. }
        | CollectionOp::ArrayConcat { packet, .. } | CollectionOp::ArrayReplace { packet, .. }
        | CollectionOp::ArrayRemove { packet, .. } | CollectionOp::ArrayDifference { packet, .. }
        | CollectionOp::MapNew { packet, .. } | CollectionOp::MapMerge { packet, .. }
        | CollectionOp::MapDifference { packet, .. } | CollectionOp::BuilderPush { packet, .. }
        | CollectionOp::BuilderAppend { packet, .. } | CollectionOp::BuilderAppendRange { packet, .. }
        | CollectionOp::BuilderFinish { packet, .. } | CollectionOp::BuilderAbandon { packet, .. }
        | CollectionOp::MapBuilderPut { packet, .. } | CollectionOp::MapBuilderAppend { packet, .. }
        | CollectionOp::MapBuilderFinish { packet, .. } | CollectionOp::MapBuilderAbandon { packet, .. } => *packet,
        CollectionOp::ArrayLen { .. } | CollectionOp::ArrayGet { .. } | CollectionOp::ArrayJoin { .. }
        | CollectionOp::ArrayRetag { .. } | CollectionOp::MapLen { .. } | CollectionOp::MapHas { .. }
        | CollectionOp::MapGet { .. } | CollectionOp::MapKeys { .. } | CollectionOp::MapValues { .. }
        | CollectionOp::MapKeyAt { .. } | CollectionOp::MapValueAt { .. } | CollectionOp::MapRetag { .. }
        | CollectionOp::BuilderNew { .. } | CollectionOp::MapBuilderNew { .. } => None,
    }
}

/// Values a builder transition inserts into its buffer.
pub(super) fn inserted(operation: &CollectionOp) -> Vec<ValueId> {
    match operation {
        CollectionOp::BuilderPush { value, .. } => vec![*value],
        CollectionOp::BuilderAppend { array, .. } => vec![*array],
        CollectionOp::MapBuilderPut { key, value, .. } => vec![*key, *value],
        CollectionOp::MapBuilderAppend { map, .. } => vec![*map],
        _ => vec![],
    }
}

/// The linear buffer a builder operation consumes. Its High name is restored
/// on the checked unwind edge, where High passes the pre-instruction buffer.
pub(super) fn consumed_buffer(operation: &Operation) -> Option<ValueId> {
    match operation {
        Operation::Collection(CollectionOp::BuilderPush { buffer, .. }
            | CollectionOp::BuilderAppend { buffer, .. } | CollectionOp::BuilderAppendRange { buffer, .. }
            | CollectionOp::BuilderFinish { buffer, .. } | CollectionOp::BuilderAbandon { buffer, .. }
            | CollectionOp::MapBuilderPut { buffer, .. } | CollectionOp::MapBuilderAppend { buffer, .. }
            | CollectionOp::MapBuilderFinish { buffer, .. } | CollectionOp::MapBuilderAbandon { buffer, .. }) => {
            Some(*buffer)
        }
        _ => None,
    }
}
