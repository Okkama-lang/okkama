//! Static emitted-site census, collected after dead landings are removed.
use super::*;
use std::collections::BTreeSet;

pub(super) fn collect(output: &mut OwnOutput, budget: &mut Budget<'_>) -> Result<(), OwnError> {
    let mut census = DynamicCensus::default();
    let mut journals = BTreeSet::new();
    let mut partitions = 0;
    for (function, data) in output.module.functions.iter() {
        let Some(body) = &data.body else { continue };
        budget.memory((body.insts.len() + body.blocks.len()) * 32)?;
        for (_, op) in summaries::operations(body) {
            budget.work(1)?;
            match op {
                Operation::Ownership(OwnershipOp::Dynamic { reason: DynamicReason::Merge, .. }) => {
                    return Err(flow_error("P1 has no emitted Merge sites"));
                }
                Operation::Ownership(OwnershipOp::Dynamic { reason, .. }) => {
                    budget.memory(192)?;
                    census.add(function, *reason, 1);
                }
                Operation::Ownership(OwnershipOp::ScopeSplit { .. } | OwnershipOp::SplitChecked { .. }) => {
                    partitions += 1;
                }
                Operation::Ownership(OwnershipOp::Bind { binding } | OwnershipOp::Rebind { binding }) => {
                    if journals.insert((function, binding.original_scope)) {
                        budget.memory(64)?;
                    }
                }
                Operation::Ownership(OwnershipOp::Stage { stage }) if journals.insert((function, stage.action)) => {
                    budget.memory(64)?;
                }
                _ => {}
            }
        }
    }
    output.stats.emitted_dynamic_sites = [DynamicReason::Atomic, DynamicReason::Cell, DynamicReason::Channel,
        DynamicReason::Group, DynamicReason::Any, DynamicReason::Closure].iter()
        .map(|reason| census.module_count(*reason)).sum();
    output.stats.emitted_checked_splits = partitions;
    output.stats.checked_partitions = partitions;
    output.stats.splits_left_unelided = partitions;
    output.stats.splits_considered = partitions + output.stats.splits_elided;
    output.stats.known_owner_journals = journals.len();
    output.census = census;
    Ok(())
}

impl OwnStats {
    /// Versioned precision section; budgets describe compilation, never runtime.
    pub fn report(&self, census: &DynamicCensus) -> String {
        let mut report = format!("own_stats version={}\n", self.schema_version);
        for (name, reason) in [("atomic", DynamicReason::Atomic), ("cell", DynamicReason::Cell),
            ("channel", DynamicReason::Channel), ("group", DynamicReason::Group), ("any", DynamicReason::Any),
            ("closure", DynamicReason::Closure), ("merge", DynamicReason::Merge)] {
            report.push_str(&format!("dynamic.{name}={}\n", census.module_count(reason)));
        }
        report.push_str(&format!("dynamic.total={}\n", self.emitted_dynamic_sites));
        for (name, count) in [("splits.considered", self.splits_considered), ("splits.elided", self.splits_elided),
            ("splits.left_unelided", self.splits_left_unelided), ("partitions.checked", self.checked_partitions),
            ("journals.known_owner", self.known_owner_journals), ("scopes.elided", self.elided_lexical_scopes),
            ("adapters.equations", self.adapter_equations), ("adapters.materialized", self.materialized_adapters),
            ("joins.incompatible_publication", self.incompatible_publication_joins),
            ("joins.incompatible_protocol", self.incompatible_protocol_joins), ("work.charged", self.charged_work),
            ("memory.peak_auxiliary_bytes", self.peak_auxiliary_bytes)] {
            report.push_str(&format!("{name}={count}\n"));
        }
        report
    }
}
