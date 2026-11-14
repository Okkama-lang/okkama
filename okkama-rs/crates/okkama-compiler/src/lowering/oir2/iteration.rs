//! C6 traversal CFG. Every iteration is re-entered through its parent scope.
use super::*;

#[derive(Clone, Copy)]
enum Indexed {
    Range,
    Array(ir::ValueId),
    Map(ir::ValueId),
}

struct Traversal {
    exit: Join,
    state: Vec<ir::ValueId>,
    // Non-scalar results use an empty/singleton array, never a fabricated resource or undef.
    packed: bool,
}

impl BodyLowerer<'_, '_> {
    pub(super) fn for_expr(
        &mut self,
        source: &thir::Source,
        binding: thir::LocalId,
        block: &thir::Block,
        ty: Option<&Ty>,
    ) -> Result<Option<ir::ValueId>> {
        if let thir::Source::Iterating(expr) | thir::Source::Batching(expr) = source {
            let Some(value) = self.value(expr)? else {
                return Ok(None);
            };
            let value = self.take(value)?;
            // As with a let, the source belongs to the enclosing traversal, not a cursor/batch.
            self.bind_internal("$source", value)?;
            let traversal = self.traversal(ty, block.span)?;
            if matches!(source, thir::Source::Batching(_)) {
                self.batch_for(value, binding, block, &traversal)?;
            } else {
                self.cursor_for(value, binding, block, &traversal)?;
            }
            return self.finish_join(&traversal.exit);
        }
        let (source, start, end, inclusive) = match source {
            thir::Source::Range { start, end, inclusive } => {
                let Some(start) = self.value(start)? else {
                    return Ok(None);
                };
                let Some(end) = self.value(end)? else {
                    return Ok(None);
                };
                (Indexed::Range, start, end, *inclusive)
            }
            thir::Source::Array(expr) if expr.ty == Some(Ty::Range) => {
                let Some(range) = self.value(expr)? else {
                    return Ok(None);
                };
                let start = self.core(ir::CoreOp::RangeStart { value: range })?;
                let len = self.core(ir::CoreOp::RangeLen { value: range })?;
                let end = self.core(ir::CoreOp::IntAdd { left: start, right: len })?;
                (Indexed::Range, start, end, false)
            }
            thir::Source::Array(_) | thir::Source::Map(_) | thir::Source::Varargs(_) => {
                let value = match source {
                    thir::Source::Array(expr) | thir::Source::Map(expr) => {
                        let Some(value) = self.value(expr)? else {
                            return Ok(None);
                        };
                        self.take(value)?
                    }
                    thir::Source::Varargs(local) => self.env[local.0].ok_or_else(|| {
                        LowerError::invalid(block.span, "unbound variadic source")
                    })?,
                    _ => unreachable!("indexed source"),
                };
                let start = self.constant(ir::Literal::Int("0".into()))?;
                let (source, end) = if matches!(source, thir::Source::Map(_)) {
                    (Indexed::Map(value), self.collection(ir::CollectionOp::MapLen { map: value })?)
                } else {
                    (Indexed::Array(value), self.collection(ir::CollectionOp::ArrayLen { array: value })?)
                };
                (source, start, end, false)
            }
            _ => unreachable!("protocol sources handled above"),
        };
        // Preserve the P1 range validation and its failure edge.
        self.core(ir::CoreOp::RangeNew { start, end, inclusive })?;
        let traversal = self.traversal(ty, block.span)?;
        self.indexed_for(source, (start, end, inclusive), binding, block, &traversal, None)?;
        self.finish_join(&traversal.exit)
    }

    fn traversal(&mut self, ty: Option<&Ty>, span: crate::source::Span) -> Result<Traversal> {
        let value_ty = ty.map(|ty| self.ty(ty, span)).transpose()?;
        let exit = self.join(self.scope(), value_ty)?;
        let mut state = vec![];
        let packed = ty.is_some_and(|ty| !matches!(ty, Ty::Int | Ty::Bool | Ty::Str | Ty::Range));
        if let Some(ty) = ty {
            state.push(self.constant(ir::Literal::Bool(false))?);
            let initial = match ty {
                Ty::Int => self.constant(ir::Literal::Int("0".into()))?,
                Ty::Bool => self.constant(ir::Literal::Bool(false))?,
                Ty::Str => self.constant(ir::Literal::Str(String::new()))?,
                Ty::Range => {
                    let zero = self.constant(ir::Literal::Int("0".into()))?;
                    self.core(ir::CoreOp::RangeNew { start: zero, end: zero, inclusive: false })?
                }
                _ => {
                    let element = value_ty.expect("valued traversal");
                    let ty = self.b.intern_type(ir::Type::Array(element))?;
                    self.collection(ir::CollectionOp::ArrayNew { ty, values: vec![], packet: None })?
                }
            };
            state.push(initial);
        }
        Ok(Traversal { exit, state, packed })
    }

    fn iteration_state(&mut self, out: Option<ir::ValueId>, traversal: &Traversal,
        span: crate::source::Span) -> Result<Vec<ir::ValueId>> {
        if !traversal.exit.value {
            return Ok(vec![]);
        }
        let ran = self.constant(ir::Literal::Bool(true))?;
        let value = out.ok_or_else(|| LowerError::invalid(span, "valued for body is void"))?;
        let value = if traversal.packed {
            let element = self.b.body().values.get(value)?.ty;
            let ty = self.b.intern_type(ir::Type::Array(element))?;
            self.collection(ir::CollectionOp::ArrayNew { ty, values: vec![value], packet: None })?
        } else {
            value
        };
        Ok(vec![ran, value])
    }

    pub(super) fn iteration_result(&mut self, state: &[ir::ValueId], packed: bool)
        -> Result<Option<ir::ValueId>> {
        if state.is_empty() {
            return Ok(None);
        }
        self.require_iteration(state[0])?;
        let value = if packed {
            let index = self.constant(ir::Literal::Int("0".into()))?;
            self.collection(ir::CollectionOp::ArrayGet { array: state[1], index })?
        } else {
            state[1]
        };
        Ok(Some(value))
    }

    fn bind_internal(&mut self, name: &str, value: ir::ValueId) -> Result<()> {
        let ty = self.b.body().values.get(value)?.ty;
        let binding = self.b.declare_binding(ir::BindingDecl {
            name: name.into(), scope: self.scope(), ty, loc: None,
        })?;
        self.b.emit(ir::Operation::Lexical(ir::LexicalOp::Bind { binding, value }), &[])?;
        Ok(())
    }

    fn indexed_for(&mut self, source: Indexed, bounds: (ir::ValueId, ir::ValueId, bool),
        binding: thir::LocalId, block: &thir::Block, traversal: &Traversal,
        refill: Option<ir::BlockId>) -> Result<()> {
        let (start, end, inclusive) = bounds;
        let parent = self.scope();
        let exit = &traversal.exit;
        let state_types: Vec<_> = traversal.state.iter()
            .map(|value| self.b.body().values.get(*value).expect("state value").ty).collect();
        let int_ty = self.ty(&Ty::Int, block.span)?;
        let mut head_types = vec![int_ty];
        head_types.extend(&state_types);
        head_types.extend(self.local_types(&exit.locals));
        let head = self.new_block(parent, &head_types)?;
        let mut initial = vec![start];
        initial.extend(&traversal.state);
        initial.extend(self.args(&exit.locals));
        self.terminate(ir::TermKind::Br(ir::Target { block: head, args: initial }))?;
        self.enter(head, &exit.locals, 1 + traversal.state.len())?;
        let head_params = self.b.block_params(head)?.to_vec();
        let index = head_params[0];
        let state = head_params[1..1 + traversal.state.len()].to_vec();
        let more = if inclusive {
            self.core(ir::CoreOp::IntLe { left: index, right: end })?
        } else {
            self.core(ir::CoreOp::IntLt { left: index, right: end })?
        };
        let scope = self.b.declare_scope(parent, ir::ScopeKind::Iteration)?;
        let iteration = self.new_block(scope, &[])?;
        let exhausted = self.new_block(parent, &[])?;
        self.terminate(ir::TermKind::Cond {
            condition: more, yes: ir::Target { block: iteration, args: vec![] },
            no: ir::Target { block: exhausted, args: vec![] },
        })?;
        let head_env = self.env.clone();
        self.switch(exhausted)?;
        if let Some(refill) = refill {
            let mut args = state.clone();
            args.extend(self.args(&exit.locals));
            self.edge(refill, args, ir::ExitKind::Fallthrough, None)?;
        } else {
            let result = self.iteration_result(&state, traversal.packed)?;
            self.branch_join(exit, result, ir::ExitKind::Fallthrough)?;
        }
        let mut step_types = state_types;
        step_types.extend(self.local_types(&exit.locals));
        let step = self.new_block(parent, &step_types)?;
        self.env = head_env;
        self.switch(iteration)?;
        let element = match source {
            Indexed::Array(array) => self.collection(ir::CollectionOp::ArrayGet { array, index })?,
            Indexed::Map(map) => self.collection(ir::CollectionOp::MapValueAt { map, index })?,
            Indexed::Range => index,
        };
        self.bind(binding, element)?;
        let boundary = self.boundary(scope, ir::BoundaryKind::Loop)?;
        self.loops.push(LoopFrame { next: step, exit: exit.clone(), boundary, state: state.clone(),
            packed: traversal.packed });
        let out = self.block(block, exit.value)?;
        if self.current.is_some() {
            let mut args = self.iteration_state(out, traversal, block.span)?;
            args.extend(self.args(&exit.locals));
            self.edge(step, args, ir::ExitKind::Fallthrough, Some(boundary))?;
        }
        self.loops.pop();
        let has_step = self.b.body().blocks.iter().any(|(_, block)| {
            matches!(&block.term.kind, ir::TermKind::Br(target) if target.block == step)
        });
        if has_step {
            self.enter(step, &exit.locals, traversal.state.len())?;
            let step_state = self.b.block_params(step)?[..traversal.state.len()].to_vec();
            let one = self.constant(ir::Literal::Int("1".into()))?;
            let advanced = self.core(ir::CoreOp::IntAdd { left: index, right: one })?;
            let mut args = vec![advanced];
            args.extend(step_state);
            args.extend(self.args(&exit.locals));
            self.edge(head, args, ir::ExitKind::Next, None)?;
        } else {
            // No body continuation: an unreachable step has no header value.
            self.dead_block(step)?;
        }
        Ok(())
    }

    fn protocol_call(&mut self, receiver: ir::ValueId, name: &str) -> Result<ir::ValueId> {
        let ty = self.b.body().values.get(receiver)?.ty;
        let ir::Type::Role { decl, .. } = *self.b.module().types.get(ty)? else {
            return Err(LowerError::invalid(self.source.span, "protocol receiver is not a resolved role"));
        };
        let ir::DeclarationKind::Role { methods, .. } = &self.b.module().declarations.get(decl)?.kind else {
            unreachable!("role declaration");
        };
        let slot = methods.iter().position(|method| method.name == name && method.signature.params.is_empty())
            .ok_or_else(|| LowerError::invalid(self.source.span, format!("missing protocol slot {name}")))?;
        let reachable = self.b.module().declarations.iter().any(|(_, declaration)| {
            matches!(&declaration.kind, ir::DeclarationKind::Object { witnesses, .. }
                if witnesses.iter().any(|witness| witness.role == ty))
        });
        if !reachable {
            return Err(LowerError::unsupported(self.source.span, "role has no reachable implementation"));
        }
        self.sync_failure()?;
        let values = self.b.emit_schema(ir::Operation::Call(ir::CallOp::Role {
            receiver: ir::CallArgument { value: receiver, convention: ir::OperandConvention::Borrow },
            role: decl, slot: slot as u32, call: super::super::roles::call_site(vec![]),
        }))?;
        super::super::roles::record_call_effect(&mut self.b, self.current.expect("open protocol call"))?;
        values.first().copied().ok_or_else(|| LowerError::invalid(self.source.span, "void protocol slot"))
    }

    fn cursor_for(&mut self, source: ir::ValueId, binding: thir::LocalId, block: &thir::Block,
        traversal: &Traversal) -> Result<()> {
        let cursor = self.protocol_call(source, "cursor")?;
        let parent = self.scope();
        let exit = &traversal.exit;
        let mut types = vec![self.b.body().values.get(cursor)?.ty];
        types.extend(traversal.state.iter().map(|value| self.b.body().values.get(*value).unwrap().ty));
        types.extend(self.local_types(&exit.locals));
        let head = self.new_block(parent, &types)?;
        let cursor_scope = self.b.declare_scope(parent, ir::ScopeKind::Iteration)?;
        let step = self.new_block(cursor_scope, &types)?;
        let mut args = vec![cursor];
        args.extend(&traversal.state);
        args.extend(self.args(&exit.locals));
        self.terminate(ir::TermKind::Br(ir::Target { block: head, args }))?;
        self.enter(head, &exit.locals, 1 + traversal.state.len())?;
        let args = self.b.block_params(head)?.to_vec();
        self.terminate(ir::TermKind::Br(ir::Target { block: step, args }))?;
        self.enter(step, &exit.locals, 1 + traversal.state.len())?;
        let params = self.b.block_params(step)?.to_vec();
        let cursor = params[0];
        let state = params[1..1 + traversal.state.len()].to_vec();
        // The current cursor is bound before either observation, including the empty step.
        self.bind_internal("$cursor", cursor)?;
        let more = self.protocol_call(cursor, "available")?;
        let body_scope = self.b.declare_scope(cursor_scope, ir::ScopeKind::Iteration)?;
        let iteration = self.new_block(body_scope, &[])?;
        let exhausted = self.new_block(cursor_scope, &[])?;
        self.terminate(ir::TermKind::Cond {
            condition: more, yes: ir::Target { block: iteration, args: vec![] },
            no: ir::Target { block: exhausted, args: vec![] },
        })?;
        let step_env = self.env.clone();
        self.switch(exhausted)?;
        let result = self.iteration_result(&state, traversal.packed)?;
        self.branch_join(exit, result, ir::ExitKind::Fallthrough)?;
        let advance_types = types[1..].to_vec();
        // `next` and body fallthrough reach successor inside the current cursor scope.
        let advance = self.new_block(cursor_scope, &advance_types)?;
        self.env = step_env;
        self.switch(iteration)?;
        let item = self.protocol_call(cursor, "item")?;
        self.bind(binding, item)?;
        let boundary = self.boundary(body_scope, ir::BoundaryKind::Loop)?;
        self.loops.push(LoopFrame {
            next: advance, exit: exit.clone(), boundary, state, packed: traversal.packed,
        });
        let out = self.block(block, exit.value)?;
        if self.current.is_some() {
            let mut args = self.iteration_state(out, traversal, block.span)?;
            args.extend(self.args(&exit.locals));
            self.edge(advance, args, ir::ExitKind::Fallthrough, Some(boundary))?;
        }
        self.loops.pop();
        let reaches = self.b.body().blocks.iter().any(|(_, data)| {
            matches!(&data.term.kind, ir::TermKind::Br(target) if target.block == advance)
        });
        if reaches {
            self.enter(advance, &exit.locals, traversal.state.len())?;
            let state = self.b.block_params(advance)?[..traversal.state.len()].to_vec();
            let successor = self.protocol_call(cursor, "successor")?;
            let mut args = vec![successor];
            args.extend(state);
            args.extend(self.args(&exit.locals));
            // This edge closes the old cursor. Only the successor crosses the step boundary.
            self.edge(head, args, ir::ExitKind::Next, None)?;
        } else {
            self.dead_block(advance)?;
        }
        Ok(())
    }

    fn batch_for(&mut self, source: ir::ValueId, binding: thir::LocalId, block: &thir::Block,
        traversal: &Traversal) -> Result<()> {
        let parent = self.scope();
        let exit = &traversal.exit;
        let mut types: Vec<_> = traversal.state.iter().map(|value| self.b.body().values.get(*value).unwrap().ty)
            .collect();
        types.extend(self.local_types(&exit.locals));
        let refill = self.new_block(parent, &types)?;
        let batch_scope = self.b.declare_scope(parent, ir::ScopeKind::Block)?;
        let entry = self.new_block(batch_scope, &types)?;
        let mut args = traversal.state.clone();
        args.extend(self.args(&exit.locals));
        self.terminate(ir::TermKind::Br(ir::Target { block: refill, args }))?;
        self.enter(refill, &exit.locals, traversal.state.len())?;
        let args = self.b.block_params(refill)?.to_vec();
        self.terminate(ir::TermKind::Br(ir::Target { block: entry, args }))?;
        self.enter(entry, &exit.locals, traversal.state.len())?;
        let state = self.b.block_params(entry)?[..traversal.state.len()].to_vec();
        // Call inside the batch scope: even an empty/failing batch exits that scope.
        let batch = self.protocol_call(source, "batch")?;
        self.bind_internal("$batch", batch)?;
        let size = self.collection(ir::CollectionOp::ArrayLen { array: batch })?;
        let zero = self.constant(ir::Literal::Int("0".into()))?;
        let more = self.core(ir::CoreOp::IntLt { left: zero, right: size })?;
        let started = self.new_block(batch_scope, &[])?;
        let exhausted = self.new_block(batch_scope, &[])?;
        self.terminate(ir::TermKind::Cond {
            condition: more, yes: ir::Target { block: started, args: vec![] },
            no: ir::Target { block: exhausted, args: vec![] },
        })?;
        let batch_env = self.env.clone();
        self.switch(exhausted)?;
        let result = self.iteration_result(&state, traversal.packed)?;
        self.branch_join(exit, result, ir::ExitKind::Fallthrough)?;
        self.env = batch_env;
        self.switch(started)?;
        let inner = Traversal { exit: exit.clone(), state, packed: traversal.packed };
        self.indexed_for(Indexed::Array(batch), (zero, size, false), binding, block, &inner, Some(refill))
    }
}
