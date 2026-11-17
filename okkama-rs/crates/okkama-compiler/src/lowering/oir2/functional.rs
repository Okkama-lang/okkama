//! Arrow blocks stay in the enclosing action. Publication follows the body
//! boundary; source projections remain borrowed, including filter survivors.
use super::*;
use thir::FunctionalOp as F;

impl BodyLowerer<'_, '_> {
    pub(super) fn functional_expr(&mut self, functional: &thir::Functional, ty: &Ty)
        -> Result<Option<ir::ValueId>> {
        self.traverse(&functional.source, functional.bindings[0], &functional.body, Some(ty), Some(functional))
    }

    pub(super) fn empty_functional(&mut self, functional: &thir::Functional, ty: &Ty)
        -> Result<Option<ir::ValueId>> {
        if !matches!(functional.op, F::Map | F::Filter) {
            let initial = functional.initial.as_ref().expect("resolved empty functional initial");
            let Some(value) = self.value(initial)? else {
                return Ok(None);
            };
            return self.take(value).map(Some);
        }
        let ty = self.ty(ty, functional.body.span)?;
        self.collection(ir::CollectionOp::ArrayNew { ty, values: vec![], packet: None }).map(Some)
    }

    pub(super) fn functional_initial(&mut self, functional: &thir::Functional, ty: &Ty)
        -> Result<Option<usize>> {
        if matches!(functional.op, F::Map | F::Filter) {
            let ty = self.ty(ty, functional.body.span)?;
            // A map whose body always exits has array<never> as its result.
            // There can be no append: retain only its empty exhaustion result.
            if matches!(self.b.module().types.get(ty)?, ir::Type::Array(element)
                if self.b.module().types.get(*element)? == &ir::Type::Never) {
                let empty = self.collection(ir::CollectionOp::ArrayNew { ty, values: vec![], packet: None })?;
                let slot = self.env.len();
                self.env.push(Some(empty));
                return Ok(Some(slot));
            }
            return self.start_buffer(ty).map(Some);
        }
        // Source evaluation has completed. The initializer executes once, even
        // on an empty source, before entering either indexed or cursor traversal.
        let initial = functional.initial.as_ref().expect("resolved functional initial");
        let linear = linear_reduce(functional, ty);
        // #831: an empty literal start of an accumulator in a builder is the empty builder itself.
        let empty = matches!(&initial.kind, thir::ExprKind::Array(parts) if parts.is_empty())
            || matches!(&initial.kind, thir::ExprKind::Map(entries) if entries.is_empty());
        if linear.is_some() && empty {
            let ty = self.ty(ty, functional.body.span)?;
            return self.start_buffer(ty).map(Some);
        }
        let Some(value) = self.value(initial)? else {
            return Ok(None);
        };
        let value = self.take(value)?;
        if let Some(map) = linear {
            // #831: the accumulator lives in a builder, which starts with the initial value.
            let ty = self.ty(ty, functional.body.span)?;
            let slot = self.start_buffer(ty)?;
            {
                let value = self.retagged(value, ty)?;
                self.update_buffer(slot, |buffer| if map {
                    ir::CollectionOp::MapBuilderAppend { buffer, map: value, packet: None }
                } else {
                    ir::CollectionOp::BuilderAppend { buffer, array: value, packet: None }
                })?;
            }
            return Ok(Some(slot));
        }
        let slot = self.env.len();
        self.env.push(Some(value));
        Ok(Some(slot))
    }

    pub(super) fn functional_result(&mut self, functional: &thir::Functional, slot: usize)
        -> Result<ir::ValueId> {
        // A map or filter, and a reduce whose accumulator lives in a builder (#831), finish their builder.
        if matches!(functional.op, F::Map | F::Filter | F::Reduce) {
            let buffer = self.env[slot].expect("completed functional state");
            let ty = self.b.body().values.get(buffer)?.ty;
            let ty = self.b.module().types.get(ty)?;
            if !matches!(ty, ir::Type::Builder(_) | ir::Type::MapBuilder(..)) {
                return Ok(self.env[slot].take().expect("completed functional result"));
            }
            let map = matches!(ty, ir::Type::MapBuilder(..));
            self.finish_buffer(slot, map)
        } else {
            Ok(self.env[slot].take().expect("completed functional result"))
        }
    }

    /// The builder type of a `reduce` whose accumulator lives in a builder (#831): Some(true) for a map builder,
    /// Some(false) for an array builder, None for a plain accumulator.
    fn accumulator_builder(&self, functional: &thir::Functional, slot: usize) -> Result<Option<bool>> {
        if functional.op != F::Reduce {
            return Ok(None);
        }
        let state = self.env[slot].expect("functional state");
        let ty = self.b.body().values.get(state)?.ty;
        Ok(match self.b.module().types.get(ty)? {
            ir::Type::Builder(_) => Some(false),
            ir::Type::MapBuilder(..) => Some(true),
            _ => None,
        })
    }

    pub(super) fn functional_body(&mut self, functional: &thir::Functional, items: Vec<ir::ValueId>,
        slot: usize, exit: &Join) -> Result<()> {
        let accumulator = self.accumulator_builder(functional, slot)?;
        let mut values = vec![];
        if functional.op == F::Reduce && accumulator.is_none() {
            values.push(self.env[slot].expect("current accumulator"));
        }
        values.extend(&items);
        // An accumulator in a builder is not bound: the block names it only as `acc` or `acc + parts`, which
        // lowering reads as what the block adds (#831).
        let bindings = if accumulator.is_some() { &functional.bindings[1..] } else { &functional.bindings[..] };
        for (&binding, value) in bindings.iter().zip(values) {
            self.bind(binding, value)?;
        }
        let body = &functional.body;
        let parent = self.scope();
        let state_ty = self.b.body().values.get(self.env[slot].expect("functional state"))?.ty;
        let body_ty = match functional.op {
            F::Map => {
                match *self.b.module().types.get(state_ty)? {
                    ir::Type::Builder(element) | ir::Type::Array(element) => element,
                    _ => unreachable!("map array builder"),
                }
            }
            F::Reduce => match *self.b.module().types.get(state_ty)? {
                ir::Type::Builder(element) => self.b.intern_type(ir::Type::Array(element))?,
                ir::Type::MapBuilder(key, value) => self.b.intern_type(ir::Type::Map(key, value))?,
                _ => state_ty,
            },
            _ => self.ty(&Ty::Bool, body.span)?,
        };
        let completed = self.join(parent, Some(body_ty))?;
        let scope = self.b.declare_scope(parent, ir::ScopeKind::Block)?;
        let entry = self.new_block(scope, &[])?;
        self.terminate(ir::TermKind::Br(ir::Target { block: entry, args: vec![] }))?;
        self.switch(entry)?;
        if accumulator.is_some() {
            self.accumulators.push(functional.bindings[0]);
        }
        let value = self.block(body, true);
        if accumulator.is_some() {
            self.accumulators.pop();
        }
        let value = value?;
        if self.current.is_none() {
            self.dead_block(completed.block)?;
            return Ok(());
        }
        // D11: no builder mutation, accumulator update or decisive result is
        // published until the arrow body's locals and defers have left scope.
        self.branch_join(&completed, value, ir::ExitKind::Fallthrough)?;
        let value = self.finish_join(&completed)?.expect("functional body result");
        match functional.op {
            F::Map => {
                self.update_buffer(slot, |buffer| ir::CollectionOp::BuilderPush { buffer, value, packet: None })?;
            }
            F::Reduce => match accumulator {
                Some(map) => self.update_buffer(slot, |buffer| if map {
                    ir::CollectionOp::MapBuilderAppend { buffer, map: value, packet: None }
                } else {
                    ir::CollectionOp::BuilderAppend { buffer, array: value, packet: None }
                })?,
                None => self.env[slot] = Some(value),
            },
            F::Filter => self.functional_filter(slot, value, &items)?,
            F::Any | F::All | F::Find => {
                let decides = self.new_block(parent, &[])?;
                let resume = self.new_block(parent, &[])?;
                let (yes, no) = if functional.op == F::All {
                    (resume, decides)
                } else {
                    (decides, resume)
                };
                self.terminate(ir::TermKind::Cond {
                    condition: value, yes: ir::Target { block: yes, args: vec![] },
                    no: ir::Target { block: no, args: vec![] },
                })?;
                let env = self.env.clone();
                self.switch(decides)?;
                let found = functional.found.as_deref().expect("resolved decisive result");
                if let Some(result) = self.value(found)? {
                    self.env[slot] = Some(result);
                    // This edge bypasses both the next body and cursor.successor.
                    self.branch_join(exit, None, ir::ExitKind::Fallthrough)?;
                }
                self.env = env;
                self.switch(resume)?;
            }
        }
        Ok(())
    }

    fn functional_filter(&mut self, slot: usize, keep: ir::ValueId, items: &[ir::ValueId]) -> Result<()> {
        let join = self.join(self.scope(), None)?;
        let insert = self.new_block(self.scope(), &[])?;
        let skip = self.new_block(self.scope(), &[])?;
        self.terminate(ir::TermKind::Cond {
            condition: keep, yes: ir::Target { block: insert, args: vec![] },
            no: ir::Target { block: skip, args: vec![] },
        })?;
        let env = self.env.clone();
        self.switch(insert)?;
        // Do not take() items: a binding or projection gives no right to steal
        // resources owned by an outer array, map, or cursor source.
        self.update_buffer(slot, |buffer| match items {
            [key, value] => ir::CollectionOp::MapBuilderPut { buffer, key: *key, value: *value, packet: None },
            [value] => ir::CollectionOp::BuilderPush { buffer, value: *value, packet: None },
            _ => unreachable!("resolved source item arity"),
        })?;
        self.branch_join(&join, None, ir::ExitKind::Fallthrough)?;
        self.env = env;
        self.switch(skip)?;
        self.branch_join(&join, None, ir::ExitKind::Fallthrough)?;
        self.finish_join(&join)?;
        Ok(())
    }
}

/// Whether a `reduce` of result type `ty` keeps its accumulator in a builder (#243 on OIR-1, #831 here):
/// Some(map) when the block uses the accumulator only as `acc` or the left end of `acc + parts` and the result is
/// an array, or a map that starts from `[:]` and grows by literals.
fn linear_reduce(functional: &thir::Functional, ty: &Ty) -> Option<bool> {
    let (Some(&acc), Some(initial)) = (functional.bindings.first(), functional.initial.as_ref()) else {
        return None;
    };
    if functional.op != F::Reduce {
        return None;
    }
    let map = match ty {
        Ty::Array(element) if !matches!(**element, Ty::Never) => false,
        Ty::Map(key, _) if !matches!(**key, Ty::Never)
            && matches!(&initial.kind, thir::ExprKind::Map(entries) if entries.is_empty()) => true,
        _ => return None,
    };
    thir::linear_accumulator(&functional.body, acc, map).then_some(map)
}
