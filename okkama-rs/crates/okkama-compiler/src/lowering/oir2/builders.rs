//! Linear High buffers. Synthetic environment slots follow the ordinary CFG;
//! every failing site observes an entry-visible current version (H5).
use super::body::BodyLowerer;
use super::Result;
use crate::analysis::thir;
use crate::oir2 as ir;

pub(super) const STREAM_LIMIT: usize = 8;

impl BodyLowerer<'_, '_> {
    fn live_buffers(&self) -> Vec<usize> {
        self.buffers.iter().copied().filter(|&slot| self.env.get(slot).and_then(|value| *value).is_some_and(|value| {
            self.b.body().values.get(value).is_ok_and(|value| {
                matches!(self.b.module().types.get(value.ty), Ok(ir::Type::Builder(_) | ir::Type::MapBuilder(..)))
            })
        })).collect()
    }

    fn abandon_op(&self, buffer: ir::ValueId) -> Result<ir::CollectionOp> {
        let ty = self.b.body().values.get(buffer)?.ty;
        Ok(if matches!(self.b.module().types.get(ty)?, ir::Type::MapBuilder(..)) {
            ir::CollectionOp::MapBuilderAbandon { buffer, packet: None }
        } else {
            ir::CollectionOp::BuilderAbandon { buffer, packet: None }
        })
    }

    pub(super) fn abandon_except(&mut self, retained: &[usize]) -> Result<()> {
        for slot in self.live_buffers().into_iter().rev() {
            if !retained.contains(&slot) {
                let buffer = self.env[slot].take().expect("live buffer");
                self.b.emit_schema(ir::Operation::Collection(self.abandon_op(buffer)?))?;
            }
        }
        Ok(())
    }

    pub(super) fn sync_buffer_failure(&mut self) -> Result<()> {
        let live = self.live_buffers();
        let block = self.current.expect("open buffer failure source");
        let Some(unwind) = self.b.body().blocks.get(block)?.unwind.clone() else {
            return Ok(());
        };
        let had_cleanup = self.buffer_unwinds.contains_key(&unwind.target.block);
        let mut target = unwind.target;
        let base_count = self.buffer_unwinds.get(&target.block).map(|&(_, count)| count)
            .unwrap_or(target.args.len());
        // A catch inside the building expression carries its outer buffers on.
        // Only buffers whose construction is being exited need abandonment.
        let slots: Vec<_> = live.into_iter().filter(|&slot| {
            !target.args[..base_count].contains(&self.env[slot].expect("live buffer"))
        }).collect();
        if let Some(&(base, count)) = self.buffer_unwinds.get(&target.block) {
            let buffers = self.args(&slots);
            if target.args[count..] == buffers
                && target.args.iter().all(|arg| self.b.block_params(block).is_ok_and(|p| p.contains(arg))) {
                return Ok(());
            }
            target.block = base;
            target.args.truncate(count);
        }
        if slots.is_empty() && !had_cleanup {
            return Ok(());
        }
        let locals = self.locals();
        let args = self.args(&locals);
        let data = self.b.body().blocks.get(block)?;
        let mut required = target.args.clone();
        required.extend(self.args(&slots));
        if !data.insts.is_empty() || !required.iter().all(|arg| data.params.contains(arg)) {
            let next = self.new_block(self.scope(), &self.local_types(&locals))?;
            let params = self.b.block_params(next)?.to_vec();
            for arg in &mut target.args {
                if let Some(index) = args.iter().position(|value| value == arg) {
                    *arg = params[index];
                }
            }
            self.b.terminate(ir::TermKind::Br(ir::Target { block: next, args }))?;
            self.b.switch_to(next)?;
            self.current = Some(next);
            for (&slot, &value) in locals.iter().zip(&params) {
                self.env[slot] = Some(value);
            }
        }
        if slots.is_empty() {
            self.b.set_unwind(Some(ir::BlockUnwind { target }))?;
            return Ok(());
        }
        let source = self.current.expect("entry-visible buffer source");
        let count = target.args.len();
        let mut args = target.args.clone();
        args.extend(self.args(&slots));
        let mut types = args.iter().map(|&value| self.b.body().values.get(value).map(|v| v.ty))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let failure = self.b.intern_type(ir::Type::Service(ir::ServiceType::Failure))?;
        types.push(failure);
        let cleanup = self.new_block(self.scope(), &types)?;
        self.b.switch_to(cleanup)?;
        let params = self.b.block_params(cleanup)?.to_vec();
        self.b.set_unwind(Some(ir::BlockUnwind {
            target: ir::Target { block: target.block, args: params[..count].to_vec() },
        }))?;
        for &buffer in params[count..params.len() - 1].iter().rev() {
            self.b.emit_schema(ir::Operation::Collection(self.abandon_op(buffer)?))?;
        }
        self.b.terminate(ir::TermKind::Raise(*params.last().expect("failure")))?;
        self.buffer_unwinds.insert(cleanup, (target.block, count));
        self.b.switch_to(source)?;
        self.b.set_unwind(Some(ir::BlockUnwind { target: ir::Target { block: cleanup, args } }))?;
        Ok(())
    }

    pub(super) fn start_buffer(&mut self, ty: ir::TypeId) -> Result<usize> {
        let op = match *self.b.module().types.get(ty)? {
            ir::Type::Array(element) => ir::CollectionOp::BuilderNew { element },
            ir::Type::Map(key, value) => ir::CollectionOp::MapBuilderNew { key, value },
            _ => unreachable!("collection buffer type"),
        };
        let buffer = self.collection(op)?;
        let slot = self.env.len();
        self.env.push(Some(buffer));
        if !self.buffers.contains(&slot) {
            self.buffers.push(slot);
        }
        Ok(slot)
    }

    pub(super) fn update_buffer(&mut self, slot: usize, op: impl FnOnce(ir::ValueId) -> ir::CollectionOp)
        -> Result<()> {
        self.sync_failure()?;
        let buffer = self.env[slot].expect("current buffer");
        let next = self.b.emit_schema(ir::Operation::Collection(op(buffer)))?[0];
        self.env[slot] = Some(next);
        Ok(())
    }

    pub(super) fn finish_buffer(&mut self, slot: usize, map: bool) -> Result<ir::ValueId> {
        self.sync_failure()?;
        let buffer = self.env[slot].take().expect("current buffer");
        let op = if map {
            ir::CollectionOp::MapBuilderFinish { buffer, packet: None }
        } else {
            ir::CollectionOp::BuilderFinish { buffer, packet: None }
        };
        Ok(self.b.emit_schema(ir::Operation::Collection(op))?[0])
    }

    pub(super) fn stream_array(&mut self, parts: &[thir::ArrayPart], ty: ir::TypeId)
        -> Result<Option<ir::ValueId>> {
        let slot = self.start_buffer(ty)?;
        // Bound temporary operand groups while keeping a large literal streamed.
        // Small/range forms retain individual pushes and append.range.
        if parts.len() >= STREAM_LIMIT * 2
            && parts.iter().all(|part| matches!(part, thir::ArrayPart::Value(_))) {
            for chunk in parts.chunks(STREAM_LIMIT) {
                let mut values = vec![];
                for part in chunk {
                    let thir::ArrayPart::Value(expr) = part else {
                        unreachable!("scalar literal chunk");
                    };
                    let Some(value) = self.value(expr)? else {
                        return Ok(None);
                    };
                    values.push(self.take(value)?);
                }
                let array = self.collection(ir::CollectionOp::ArrayNew { values, ty, packet: None })?;
                self.update_buffer(slot, |buffer| ir::CollectionOp::BuilderAppend { buffer, array, packet: None })?;
            }
            return self.finish_buffer(slot, false).map(Some);
        }
        for part in parts {
            match part {
                thir::ArrayPart::Value(expr) => {
                    let Some(value) = self.value(expr)? else {
                        return Ok(None);
                    };
                    let value = self.take(value)?;
                    self.update_buffer(slot, |buffer| ir::CollectionOp::BuilderPush { buffer, value, packet: None })?;
                }
                thir::ArrayPart::Range { start, end, inclusive } => {
                    let Some(start) = self.value(start)? else {
                        return Ok(None);
                    };
                    let Some(end) = self.value(end)? else {
                        return Ok(None);
                    };
                    self.update_buffer(slot, |buffer| ir::CollectionOp::BuilderAppendRange {
                        buffer, start, end, inclusive: *inclusive, packet: None,
                    })?;
                }
            }
        }
        self.finish_buffer(slot, false).map(Some)
    }

    pub(super) fn stream_collections(&mut self, parts: &[&thir::Expr], ty: ir::TypeId, map: bool)
        -> Result<Option<ir::ValueId>> {
        let slot = self.start_buffer(ty)?;
        for expr in parts {
            let Some(value) = self.value(expr)? else {
                return Ok(None);
            };
            let value = self.take(value)?;
            let value = if self.b.body().values.get(value)?.ty != ty {
                self.collection(if map {
                    ir::CollectionOp::MapRetag { map: value, ty }
                } else {
                    ir::CollectionOp::ArrayRetag { array: value, ty }
                })?
            } else {
                value
            };
            self.update_buffer(slot, |buffer| if map {
                ir::CollectionOp::MapBuilderAppend { buffer, map: value, packet: None }
            } else {
                ir::CollectionOp::BuilderAppend { buffer, array: value, packet: None }
            })?;
        }
        self.finish_buffer(slot, map).map(Some)
    }

    pub(super) fn stream_map(&mut self, entries: &[(thir::Expr, thir::Expr)], ty: ir::TypeId)
        -> Result<Option<ir::ValueId>> {
        let slot = self.start_buffer(ty)?;
        for (key, value) in entries {
            let Some(key) = self.value(key)? else {
                return Ok(None);
            };
            let key = self.take(key)?;
            let Some(value) = self.value(value)? else {
                return Ok(None);
            };
            let value = self.take(value)?;
            self.update_buffer(slot, |buffer| ir::CollectionOp::MapBuilderPut { buffer, key, value, packet: None })?;
        }
        self.finish_buffer(slot, true).map(Some)
    }
}
