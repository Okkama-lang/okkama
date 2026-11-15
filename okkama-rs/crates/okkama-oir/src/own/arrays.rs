//! Plain array specialization only. Managed/residual collections need the later collection producer.
use super::*;

pub(super) fn supported(
    module: &Module,
    body: &Body,
    analysis: &OwnAnalysis,
    operation: &Operation,
) -> bool {
    let ty = match operation {
        Operation::Collection(CollectionOp::ArrayNew { ty, packet: None, .. }) => Some(*ty),
        Operation::Collection(CollectionOp::ArrayLen { array } | CollectionOp::ArrayGet { array, .. }) => {
            body.values.get(*array).ok().map(|value| value.ty)
        }
        _ => None,
    };
    ty.is_some_and(|ty| matches!(module.types.get(ty), Ok(Type::Array(_)))
        && analysis.types.get(&ty).is_some_and(|facts| {
            facts.class == Classification::Plain
                && facts.certificate == Some(PlainCertificate::ClosedGraph { ty })
        }))
}
