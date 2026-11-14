//! Producer facts only. No callable contract, summary, ownership decision or IR operation is emitted.
use std::collections::HashMap;

use crate::analysis::{Analysis, thir, ty};
use crate::source::Span;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct BodyKey {
    pub root: thir::BodyOwner,
    /// Closure ordinals in each enclosing lexical body, independent of spans and queue order.
    pub path: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Snapshot {
    Receiver,
    Parameter(thir::LocalId),
    Local(thir::LocalId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Capture {
    pub source: Snapshot,
    /// Receiver has no THIR local. Other captures map to the closure's local table.
    pub target: Option<thir::LocalId>,
    pub ty: ty::Ty,
}

#[derive(Clone, Debug)]
pub(super) struct DiscoveredBody {
    pub key: BodyKey,
    /// Lexical receiver authority, even when no receiver snapshot is needed.
    pub receiver: Option<ty::Ty>,
    /// Receiver snapshot first when used, then THIR captures in their original order.
    pub captures: Vec<Capture>,
    pub body: thir::Body,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OpenCallSite {
    pub body: BodyKey,
    pub module: usize,
    pub span: Span,
    pub ordinal: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OpenCallable {
    /// A concrete Ty::Action, including nested parameter/result signatures.
    pub ty: ty::Ty,
    pub sites: Vec<OpenCallSite>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum BoundTarget {
    Method { object: ty::ObjectId, method: usize },
    Role { role: ty::RoleId, slot: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct BoundAction {
    pub body: BodyKey,
    pub span: Span,
    pub receiver: ty::Ty,
    pub target: BoundTarget,
}

#[derive(Debug, Default)]
pub(super) struct Inventory {
    pub test_mode: bool,
    pub bodies: Vec<DiscoveredBody>,
    pub bound: Vec<BoundAction>,
    /// First-discovery order, deduplicated by the full concrete structural type.
    pub open_calls: Vec<OpenCallable>,
}

#[derive(Debug)]
pub(super) struct Prepared<'a> {
    pub analysis: Analysis<'a>,
    pub callables: Inventory,
}

impl Inventory {
    pub fn open_call(&mut self, ty: ty::Ty, site: OpenCallSite) {
        if let Some(entry) = self.open_calls.iter_mut().find(|entry| entry.ty == ty) {
            entry.sites.push(site);
        } else {
            self.open_calls.push(OpenCallable { ty, sites: vec![site] });
        }
    }

    /// Reattach concrete bodies after the worklist has finished, from deepest paths upward.
    pub fn attach(&mut self, roots: &mut [thir::Body]) {
        let indices: HashMap<_, _> = self.bodies.iter().enumerate()
            .map(|(index, entry)| (entry.key.clone(), index)).collect();
        let mut order: Vec<_> = (0..self.bodies.len()).collect();
        order.sort_by_key(|&index| std::cmp::Reverse(self.bodies[index].key.path.len()));
        for index in order {
            let key = self.bodies[index].key.clone();
            let mut body = self.bodies[index].body.clone();
            self.attach_body(&key, &mut body, &indices);
            self.bodies[index].body = body;
        }
        for body in roots {
            self.attach_body(&BodyKey { root: body.owner.clone(), path: vec![] }, body, &indices);
        }
    }

    fn attach_body(&self, key: &BodyKey, body: &mut thir::Body, indices: &HashMap<BodyKey, usize>) {
        let mut ordinal = 0;
        super::walk::block_mut(&mut body.block, &mut |expr| {
            if let thir::ExprKind::Closure { body, .. } = &mut expr.kind {
                let mut child = key.clone();
                child.path.push(ordinal);
                ordinal += 1;
                let concrete = &self.bodies[indices[&child]];
                **body = concrete.body.clone();
            }
        });
    }
}
