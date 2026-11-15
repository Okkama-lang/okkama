//! D20 §2.6: Low replaces the continuation of a suspending close with continuations of its suspending
//! cleanup steps and drops the bracket of every step that cannot suspend.
//!
//! In 0.6.0 no cleanup step of a plain async close suspends:
//! - `cleanup.settle` never waits (async ABI D20 "not suspending", D24.2, OQ-4), so it carries no bracket;
//! - async destroy and async defer are rejected by eligibility (slices D1 and R2).
//!
//! So the coro bracket of every close goes away: its `frame.save` and `frame.resume` are removed and the
//! resumed versions of the frame and of the saved values are replaced by the values that were saved.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

/// The close bracketed by a `frame.save`, if its site is a close.
fn bracketed_close(body: &Body, block: BlockId, position: usize) -> Result<Option<Site>, LowError> {
    let data = body.blocks.get(block)?;
    let site = match data.insts.get(position + 1) {
        Some(next) => {
            if !close(&body.insts.get(*next)?.op) {
                return Ok(None);
            }
            Site::Instruction(*next)
        }
        None => match &data.term.kind {
            TermKind::InvokeOwn { op, .. } if close(op) => Site::Terminator(block),
            _ => return Ok(None),
        },
    };
    Ok(Some(site))
}

fn close(op: &Operation) -> bool {
    matches!(op, Operation::Ownership(OwnershipOp::ScopeEnd { .. } | OwnershipOp::ScopeUnwind { .. }))
}

/// A suspension site: an instruction, or the terminator of a block (D20 forms (b) and (a)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Site {
    Instruction(InstId),
    Terminator(BlockId),
}

/// The resumed values of one `frame.resume` mapped to the values its `frame.save` stored.
fn substitutions(
    body: &Body, save: &Instruction, resume: InstId, continuation: u32, map: &mut BTreeMap<ValueId, ValueId>,
) -> Result<(), LowError> {
    let invalid = |message: &str| LowError::InvalidInput { function: None, message: message.into() };
    let Operation::Async(AsyncOp::FrameSave { frame, slots: saved, .. }) = &save.op else {
        return Err(invalid("a bracket starts with frame.save"));
    };
    let saved_frame = *save.results.first().ok_or_else(|| invalid("frame.save defines the saved frame"))?;
    let instruction = body.insts.get(resume)?;
    let Operation::Async(AsyncOp::FrameResume { frame: resumed, continuation: other, slots }) = &instruction.op
    else {
        return Err(invalid("every successor of a suspending close starts with its frame.resume (D20)"));
    };
    if *resumed != saved_frame || *other != continuation || instruction.results.len() != slots.len() + 1 {
        return Err(invalid("frame.resume restores the frame and slots of its frame.save (D20)"));
    }
    map.insert(instruction.results[0], *frame);
    for (slot, value) in slots.iter().zip(&instruction.results[1..]) {
        let original = saved.iter().find(|item| item.slot == slot.slot)
            .ok_or_else(|| invalid("frame.resume restores a slot that its frame.save did not store (D21)"))?;
        map.insert(*value, original.value);
    }
    Ok(())
}

struct Substitute<'a>(&'a BTreeMap<ValueId, ValueId>);
impl crate::visit::VisitorMut for Substitute<'_> {
    fn value(&mut self, value: &mut ValueId) {
        // A value saved at one narrowed close may itself be the resumed version of another one; in SSA the
        // chain is acyclic, so it has at most one link per substitution.
        for _ in 0..self.0.len() {
            let Some(original) = self.0.get(value) else {
                break;
            };
            *value = *original;
        }
    }
}

/// Remove the bracket of every close and return the continuations it named.
pub(super) fn closes(body: &mut Body, budget: &mut Budget<'_>) -> Result<BTreeSet<u32>, LowError> {
    let mut removed = BTreeSet::new();
    let mut map = BTreeMap::new();
    let blocks: Vec<BlockId> = body.blocks.iter().map(|(id, _)| id).collect();
    for block in blocks {
        let mut position = 0;
        while position < body.blocks.get(block)?.insts.len() {
            budget.step(1)?;
            let id = body.blocks.get(block)?.insts[position];
            let save = body.insts.get(id)?.clone();
            let Operation::Async(AsyncOp::FrameSave { continuation, .. }) = save.op else {
                position += 1;
                continue;
            };
            let Some(site) = bracketed_close(body, block, position)? else {
                position += 1;
                continue;
            };
            match site {
                Site::Instruction(_) => {
                    let message = "a close bracket ends with frame.resume";
                    let resume = *body.blocks.get(block)?.insts.get(position + 2)
                        .ok_or_else(|| LowError::InvalidInput { function: None, message: message.into() })?;
                    substitutions(body, &save, resume, continuation, &mut map)?;
                    let data = body.blocks.get_mut(block)?;
                    data.insts.remove(position + 2);
                    data.insts.remove(position);
                }
                Site::Terminator(_) => {
                    let mut targets: Vec<BlockId> = graph::successors(body, block)?.into_iter().collect();
                    targets.dedup();
                    for target in targets {
                        let resume = *body.blocks.get(target)?.insts.first().ok_or_else(|| LowError::InvalidInput {
                            function: None, message: "a close successor starts with frame.resume (D20)".into(),
                        })?;
                        substitutions(body, &save, resume, continuation, &mut map)?;
                        body.blocks.get_mut(target)?.insts.remove(0);
                    }
                    body.blocks.get_mut(block)?.insts.remove(position);
                }
            }
            removed.insert(continuation);
        }
    }
    budget.step(map.len())?;
    crate::visit::rewrite_uses(body, &mut Substitute(&map));
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(source: &str) -> Module {
        text::parse(source).unwrap_or_else(|error| panic!("{error}"))
    }

    fn main(module: &mut Module) -> &mut Body {
        module.functions.iter_mut().find(|(_, function)| function.symbol == "main").unwrap().1.body.as_mut().unwrap()
    }

    fn brackets(body: &Body) -> Vec<u32> {
        let mut continuations: Vec<u32> = body.blocks.iter().flat_map(|(_, block)| block.insts.clone())
            .filter_map(|inst| match body.insts.get(inst).unwrap().op {
                Operation::Async(AsyncOp::FrameSave { continuation, .. }) => Some(continuation),
                _ => None,
            }).collect();
        continuations.sort_unstable();
        continuations
    }

    /// Both close brackets of plain await (form (a) at the end, form (b) at the unwind) go; the brackets of
    /// the await and of `await.unwind` stay; the frame published after the close is the frame before it.
    #[test]
    fn close_brackets_go_and_await_brackets_stay() {
        let mut module = parsed(include_str!("../../tests/fixtures/async/coro/async-plain-await.oir"));
        let body = main(&mut module);
        assert_eq!(brackets(body), [1, 2, 3, 4]);
        let options = LowOptions::default();
        let mut budget = Budget { options: &options, steps: 0, blocks: 0, roots: 0 };
        let removed = closes(body, &mut budget).unwrap();
        assert_eq!(removed.into_iter().collect::<Vec<_>>(), [2, 4]);
        assert_eq!(brackets(body), [1, 3]);
        let resumes: Vec<u32> = body.blocks.iter().flat_map(|(_, block)| block.insts.clone())
            .filter_map(|inst| match body.insts.get(inst).unwrap().op {
                Operation::Async(AsyncOp::FrameResume { continuation, .. }) => Some(continuation),
                _ => None,
            }).collect();
        assert!(resumes.iter().all(|continuation| [0, 1, 3].contains(continuation)), "{resumes:?}");
        // Every publication now consumes a frame defined by a remaining resume.
        for (_, block) in body.blocks.iter() {
            for inst in &block.insts {
                let Operation::Async(AsyncOp::FuturePublish { frame, .. }) = body.insts.get(*inst).unwrap().op else {
                    continue;
                };
                let Definition::InstResult { inst: definition, .. } = body.values.get(frame).unwrap().def else {
                    panic!("a frame is an instruction result");
                };
                let placed = body.blocks.iter().any(|(_, block)| block.insts.contains(&definition));
                assert!(placed, "a publication reads a removed resume");
            }
        }
    }

    /// A resume that restores a slot its save did not store is invalid input.
    #[test]
    fn resume_of_an_unsaved_slot_is_invalid() {
        let source = include_str!("../../tests/fixtures/async/coro/coro-nested-cold-await.oir");
        let mut module = parsed(source);
        let body = main(&mut module);
        let (save, _) = body.insts.iter().find(|(_, inst)| matches!(inst.op,
            Operation::Async(AsyncOp::FrameSave { continuation: 2, .. }))).unwrap();
        let resume = body.insts.iter().find(|(_, inst)| matches!(inst.op,
            Operation::Async(AsyncOp::FrameResume { continuation: 2, .. }))).map(|(id, _)| id).unwrap();
        let save = body.insts.get(save).unwrap().clone();
        let mut forged = body.insts.get(resume).unwrap().clone();
        let Operation::Async(AsyncOp::FrameResume { slots, .. }) = &mut forged.op else {
            unreachable!();
        };
        let ty = body.values.get(forged.results[0]).unwrap().ty;
        slots.push(SavedSlot { slot: 7, ty, linear: true });
        forged.results.push(forged.results[0]);
        *body.insts.get_mut(resume).unwrap() = forged;
        let mut map = BTreeMap::new();
        let error = substitutions(body, &save, resume, 2, &mut map).unwrap_err();
        assert!(matches!(error, LowError::InvalidInput { .. }), "{error:?}");
    }
}
