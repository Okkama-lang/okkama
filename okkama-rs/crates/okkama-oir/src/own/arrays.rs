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

/// Whether the body creates a builder; only then are builder owners computed and charged.
pub(super) fn creates_builders(body: &Body) -> bool {
    body.insts.iter().any(|(_, instruction)| matches!(instruction.op,
        Operation::Collection(CollectionOp::BuilderNew { .. } | CollectionOp::MapBuilderNew { .. })))
}

/// The lexical scope of the `builder.new` behind every High buffer version.
///
/// The builder's owner holds the journal of its elements (spec §4.1, §8): a
/// transition moves the fresh parts of inserted values into that scope, so they
/// are destroyed when the scope that created the builder closes. A buffer flows
/// only through its own transitions and edge arguments. Versions of builders made
/// in different scopes meeting at a join are refused (`join_owner`).
pub(super) fn builder_owners(module: &Module, body: &Body) -> Result<BTreeMap<ValueId, ScopeId>, OwnError> {
    let buffer = |value: ValueId| -> Result<bool, OwnError> {
        let ty = body.values.get(value).map_err(arena)?.ty;
        Ok(matches!(module.types.get(ty).map_err(arena)?, Type::Builder(_) | Type::MapBuilder(..)))
    };
    let mut owners = BTreeMap::new();
    let mut changed = true;
    while changed {
        changed = false;
        for (_, block) in body.blocks.iter() {
            for inst in &block.insts {
                let instruction = body.insts.get(*inst).map_err(arena)?;
                let owner = match &instruction.op {
                    Operation::Collection(CollectionOp::BuilderNew { .. } | CollectionOp::MapBuilderNew { .. }) => {
                        Some(block.lexical_scope)
                    }
                    op => consumed_buffer(op).and_then(|consumed| owners.get(&consumed).copied()),
                };
                let Some(owner) = owner else {
                    continue;
                };
                for result in &instruction.results {
                    if buffer(*result)? {
                        changed |= join_owner(&mut owners, *result, owner)?;
                    }
                }
            }
            for (target, args) in edge_arguments(block) {
                let params = &body.blocks.get(target).map_err(arena)?.params;
                for (argument, parameter) in args.into_iter().zip(params) {
                    let Some(owner) = argument.and_then(|argument| owners.get(&argument).copied()) else {
                        continue;
                    };
                    if buffer(*parameter)? {
                        changed |= join_owner(&mut owners, *parameter, owner)?;
                    }
                }
            }
        }
    }
    Ok(owners)
}

/// Positional High edge arguments; a produced edge result has no SSA source.
fn edge_arguments(block: &Block) -> Vec<(BlockId, Vec<Option<ValueId>>)> {
    let plain = |target: &Target| (target.block, target.args.iter().copied().map(Some).collect());
    let produced = |edge: &InvokeEdge| (edge.block, edge.args.iter().map(|arg| match arg {
        EdgeArg::Value(value) => Some(*value),
        EdgeArg::Produced(_) => None,
    }).collect());
    let mut edges: Vec<(BlockId, Vec<Option<ValueId>>)> = vec![];
    match &block.term.kind {
        TermKind::Br(target) => edges.push(plain(target)),
        TermKind::Cond { yes, no, .. } => {
            edges.push(plain(yes));
            edges.push(plain(no));
        }
        TermKind::Switch { cases, default, .. } => {
            edges.extend(cases.iter().map(|case| plain(&case.target)));
            edges.push(plain(default));
        }
        TermKind::Catch { handler, next, .. } => {
            edges.push(produced(handler));
            edges.push(plain(next));
        }
        TermKind::Lookup { found, missing, .. } => {
            edges.push(produced(found));
            edges.push(plain(missing));
        }
        _ => {}
    }
    if let Some(unwind) = &block.unwind {
        edges.push(plain(&unwind.target));
    }
    edges
}

/// Record `owner` for the buffer version `value`. The owner's journal already holds the elements pushed
/// into the version, and no transition moves a journal between scopes, so versions of builders made in
/// different scopes cannot meet at a join: the merged buffer would have no single owner.
fn join_owner(
    owners: &mut BTreeMap<ValueId, ScopeId>,
    value: ValueId,
    owner: ScopeId,
) -> Result<bool, OwnError> {
    match owners.insert(value, owner) {
        None => Ok(true),
        Some(previous) if previous == owner => Ok(false),
        Some(_) => Err(flow_error("builder versions made in different scopes meet at a join")),
    }
}

/// The scopes from which a transition in `scope` moves inserted elements to
/// `owner`: the lexical scopes strictly inside the owner on the current stack.
pub(super) fn inner_scopes(module: &Module, scope: ScopeId, owner: ScopeId) -> Result<Vec<ScopeId>, OwnError> {
    let stack = super::edges::stack(module, scope)?;
    let position = stack.iter().position(|scope| *scope == owner)
        .ok_or_else(|| flow_error("builder transition lies outside the scope of its builder.new"))?;
    Ok(stack[position + 1..].to_vec())
}
