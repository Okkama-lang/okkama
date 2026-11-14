//! Runtime adapter for legacy declarations and final intrinsic storage overrides.
use crate::heap;
use crate::word::Repr;
use crate::Error;
use super::{LayoutFacts, ObjectRuntimeLayout};

impl crate::program::Loaded {
    /// Legacy declaration adapter for S1 allocation/JIT consumers. Runtime overrides
    /// have already produced `layouts`; own destroy is independent of reachability.
    pub(crate) fn champ_object_layouts(&self, layouts: &heap::Layouts)
        -> Result<Box<[ObjectRuntimeLayout]>, Error>
    {
        let reaches = crate::failures::descriptor_flags(&self.program);
        self.program.objects.iter().zip(&layouts.objects).enumerate().map(|(index, (object, fields))| {
            let may_async_destroy = object.destroy.is_some_and(|entry| {
                self.program.functions[entry.0 as usize].is_async
            });
            let selection = fields.iter().enumerate().filter_map(|(index, &repr)| {
                (repr == Repr::Ref).then_some(index)
            }).collect();
            ObjectRuntimeLayout::new(fields.clone(), LayoutFacts {
                has_destroy: object.destroy.is_some(),
                may_reach_resource: reaches[index],
                may_async_destroy,
            }, selection)
        }).collect::<Result<Box<[_]>, _>>().map_err(|error| Error(error.to_string()))
    }
}
