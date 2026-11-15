//! D21 slot assignment: one type per slot, reused across continuations.
use crate::*;

/// D21: `linear` equals the linearity of the slot type, or is true for a frame.
pub(super) fn linear(types: &TypeTable, ty: TypeId) -> bool {
    match types.get(ty) {
        Ok(Type::Service(service)) => service.is_linear(),
        Ok(Type::Builder(_) | Type::MapBuilder(..) | Type::Frame(_)) => true,
        _ => false,
    }
}

#[derive(Default)]
pub(super) struct Slots {
    pub decls: Vec<FrameSlotDecl>,
}
impl Slots {
    /// One slot per distinct value of one continuation: the lowest slot of the value's type that this
    /// continuation does not use yet, or a new slot.
    pub(super) fn assign(&mut self, types: &TypeTable, values: &[(ValueId, TypeId)]) -> Vec<u32> {
        let mut taken: Vec<(ValueId, u32)> = Vec::new();
        let mut slots = Vec::with_capacity(values.len());
        for &(value, ty) in values {
            if let Some((_, slot)) = taken.iter().find(|(seen, _)| *seen == value) {
                slots.push(*slot);
                continue;
            }
            let reused = self.decls.iter().enumerate().find(|(index, decl)| {
                decl.ty == ty && !taken.iter().any(|(_, slot)| *slot as usize == *index)
            }).map(|(index, _)| index);
            let index = match reused {
                Some(index) => index,
                None => {
                    self.decls.push(FrameSlotDecl { ty, linear: linear(types, ty) });
                    self.decls.len() - 1
                }
            };
            let slot = u32::try_from(index).unwrap_or(u32::MAX);
            taken.push((value, slot));
            slots.push(slot);
        }
        slots
    }
}
