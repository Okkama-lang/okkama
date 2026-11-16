use super::*;

impl Universe {
    pub(super) fn infer_body(
        &self,
        module: &Module,
        function: FuncId,
        types: &BTreeMap<TypeId, TypeClassification>,
        budget: &mut Budget<'_>,
    ) -> Result<BodyInference, OwnError> {
        let declaration = module.functions.get(function).map_err(arena)?;
        let body = declaration.body.as_ref().unwrap();
        let mut summary = bottom(module, declaration);
        if module.profile == Profile::High && summary.contract.protocol.is_some() {
            normal::failures(&mut summary.contract.failures, &[
                FailureKind::Thrown, FailureKind::Program, FailureKind::Panic, FailureKind::Native,
            ]);
        }
        let mut replacements = BTreeMap::new();
        budget.memory(body.values.len() * 256 + (body.insts.len() + body.blocks.len()) * 64)?;
        let mut values = vec![normal::empty_relation(); body.values.len()];
        let entry = body.blocks.get(body.entry).map_err(arena)?;
        let mut parameter = 0u16;
        // #700, #731: parameters with deps and no obligation. A result that keeps one depends on it and owns
        // nothing of it: the parameter stays borrowed (OIR-2 §7.2, §10.2; P2.4 §4.8: storage must not become
        // ownership).
        let mut borrowed = BTreeSet::new();
        // PR-P (#749): the borrowed parameters by value, for the fields they lend. A block parameter fed by several
        // borrowed parameters lends the fields of each of them.
        let mut lent: BTreeMap<ValueId, BTreeSet<u16>> = BTreeMap::new();
        for value in &entry.params {
            let ty = body.values.get(*value).map_err(arena)?.ty;
            if !language(module, &[ty]).is_empty() {
                if !types[&ty].may_own() && types[&ty].references {
                    borrowed.insert(parameter);
                    lent.entry(*value).or_default().insert(parameter);
                }
                if types[&ty].may_own() {
                    let relation = &mut values[value.as_u32() as usize];
                    relation.aliases.push(SummarySelection::Parameter(parameter));
                    relation.external_deps.push(parameter);
                } else if types[&ty].references {
                    // A parameter with deps but no obligation (a role, cap, closure, `any` or Atomic reference):
                    // a result that retains it depends on it (P2.4 §5.2), without aliasing an owned graph.
                    values[value.as_u32() as usize].external_deps.push(parameter);
                }
                parameter = parameter.checked_add(1).ok_or_else(|| {
                    budget.fail(OwnDiagnosticKind::IncompatibleSignature, "too many language parameters")
                })?;
            }
        }
        let operations = operations(body);
        let fields = if module.profile == Profile::High {
            let graph = super::super::edges::Graph::index(module, body, budget)?;
            super::super::construction::completion_fields(module, body, &graph, budget)?
        } else {
            BTreeMap::new()
        };
        loop {
            let mut changed = false;
            for (site, op) in &operations {
                budget.work(1)?;
                if let Operation::Core(CoreOp::ObjInit { object, field, .. }) = op
                    && types[&body.values.get(*object).map_err(arena)?.ty].may_own()
                    && module.declarations.get(field.declaration).is_ok_and(|declaration| {
                        matches!(&declaration.kind, DeclarationKind::Object { fields, .. }
                            if fields[field.index as usize].name == "$pending")
                    }) {
                    let mut completed = values[object.as_u32() as usize].clone();
                    if let Site::Instruction(inst) = site
                        && let Some(fields) = fields.get(inst) {
                        for value in fields {
                            normal::relation(&mut completed, &values[value.as_u32() as usize], budget)?;
                        }
                    }
                    completed.fresh.push(site.origin(body, 0, self.strides[&function]));
                    changed |= join_value(&mut values, *object, &completed, budget)?;
                }
                let mut output = self.operation_relations(module, function, body, *site, op, &values, types, budget)?;
                // PR-P (#749): a carrier-free field of a borrowed parameter is that field, lent: the relation names
                // it, so a result that keeps it declares it. A borrowed parameter has no alias of its own to project
                // (#700, #731: it is never taken).
                if let Operation::Core(CoreOp::ObjGet { object, field }) = op
                    && let Some(lenders) = lent.get(object)
                    && let Some(relation) = output.first_mut() {
                    let parameters = language_parameters(module, body);
                    for &parameter in lenders {
                        let selection = SummarySelection::Field { parameter, field: *field };
                        if carrier_free_projection(module, types, &selection)
                            && lent_path(module, &parameters, &selection) {
                            relation.aliases.push(selection);
                        }
                    }
                }
                if let Site::Instruction(instruction) = site {
                    for (result, relation) in body.insts.get(*instruction).map_err(arena)?.results.iter().zip(&output) {
                        changed |= join_value(&mut values, *result, relation, budget)?;
                    }
                }
            }
            for (block_id, block) in body.blocks.iter() {
                budget.work(1)?;
                let mut edge = |target: &Target| -> Result<(), OwnError> {
                    for (parameter, argument) in body.blocks.get(target.block).map_err(arena)?.params.iter()
                        .zip(&target.args) {
                        // PR-P (#749): a block parameter lends the fields of every borrowed parameter that reaches it.
                        // Another value reaching it adds nothing: an alias only adds deps.
                        if let Some(lenders) = lent.get(argument).cloned() {
                            let target = lent.entry(*parameter).or_default();
                            let before = target.len();
                            target.extend(lenders);
                            changed |= target.len() != before;
                        }
                        let relation = values[argument.as_u32() as usize].clone();
                        changed |= join_value(&mut values, *parameter, &relation, budget)?;
                    }
                    Ok(())
                };
                match &block.term.kind {
                    TermKind::Br(target) => edge(target)?,
                    TermKind::Cond { yes, no, .. } => {
                        edge(yes)?;
                        edge(no)?;
                    }
                    TermKind::Switch { cases, default, .. } => {
                        for case in cases {
                            edge(&case.target)?;
                        }
                        edge(default)?;
                    }
                    TermKind::Catch { next, .. } => edge(next)?,
                    TermKind::Lookup { missing, .. } => edge(missing)?,
                    _ => {}
                }
                let mut produced = vec![];
                if let TermKind::InvokeOwn { op, .. } = &block.term.kind {
                    produced = self.operation_relations(module, function, body, Site::Terminator(block_id),
                        op, &values, types, budget)?;
                }
                let mut deferred = vec![];
                if let TermKind::InvokeDefer { function: callee, call, replacements, .. } = &block.term.kind {
                    let actuals: Vec<_> = call.args.iter().map(|arg| values[arg.value.as_u32() as usize].clone())
                        .collect();
                    for replacement in replacements {
                        let relation = self.replacement_relation(function, body, Site::Terminator(block_id),
                            *callee, &replacement.label, &actuals, budget)?;
                        let labels = module.functions.get(*callee).map_err(arena)?.attrs.iter().find_map(|attr| {
                            if let FunctionAttr::DeferEntry { replacements } = attr { Some(replacements) } else { None }
                        }).ok_or_else(|| budget.fail(OwnDiagnosticKind::MissingBoundary, "D8 entry has no labels"))?;
                        let has_value = labels.iter().find(|label| label.label == replacement.label)
                            .is_some_and(|label| label.result.is_some());
                        let mut slots = vec![];
                        if has_value {
                            slots.push(relation);
                        }
                        slots.extend([normal::empty_relation(), normal::empty_relation()]);
                        deferred.push(slots);
                    }
                }
                let mut invoke = |target: &InvokeEdge, produced: &[ResultRelation]| -> Result<(), OwnError> {
                    for (parameter, argument) in body.blocks.get(target.block).map_err(arena)?.params.iter()
                        .zip(&target.args) {
                        let relation = match argument {
                            EdgeArg::Value(value) => values[value.as_u32() as usize].clone(),
                            EdgeArg::Produced(result) => {
                                produced.get(usize::from(result.slot)).cloned().unwrap_or_else(normal::empty_relation)
                            }
                        };
                        changed |= join_value(&mut values, *parameter, &relation, budget)?;
                    }
                    Ok(())
                };
                match &block.term.kind {
                    TermKind::InvokeOwn { normal, unwind, .. } => {
                        invoke(normal, &produced)?;
                        invoke(unwind, &[])?;
                    }
                    TermKind::InvokeDefer { continue_edge, replacements, unwind, .. } => {
                        invoke(continue_edge, &[])?;
                        for (replacement, produced) in replacements.iter().zip(&deferred) {
                            invoke(&replacement.edge, produced)?;
                        }
                        if let Some(unwind) = unwind {
                            invoke(unwind, &[])?;
                        }
                    }
                    TermKind::Catch { handler, .. } => invoke(handler, &[])?,
                    TermKind::Lookup { .. } => {
                        // Lookup projections select elements, preserving full
                        // actual deps. Its one produced slot is handled below.
                    }
                    _ => {}
                }
                if let TermKind::Lookup { collection, found, .. } = &block.term.kind {
                    for (parameter, argument) in body.blocks.get(found.block).map_err(arena)?.params.iter()
                        .zip(&found.args) {
                        let relation = match argument {
                            EdgeArg::Value(value) => values[value.as_u32() as usize].clone(),
                            EdgeArg::Produced(_) => values[collection.as_u32() as usize].clone(),
                        };
                        changed |= join_value(&mut values, *parameter, &relation, budget)?;
                    }
                }
                if let Some(unwind) = &block.unwind {
                    for (parameter, argument) in body.blocks.get(unwind.target.block).map_err(arena)?.params.iter()
                        .zip(&unwind.target.args) {
                        let relation = values[argument.as_u32() as usize].clone();
                        changed |= join_value(&mut values, *parameter, &relation, budget)?;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        // Elaboration splits only against the state of a scope it keeps. A scope keeps no state when its
        // inventory makes it eligible for elision, unless it is the root of an async action or a defer helper.
        let mut site_scopes = BTreeMap::new();
        for (block_id, block) in body.blocks.iter() {
            budget.work(1 + block.insts.len())?;
            site_scopes.insert(Site::Terminator(block_id), block.lexical_scope);
            site_scopes.extend(block.insts.iter().map(|inst| (Site::Instruction(*inst), block.lexical_scope)));
        }
        budget.memory(site_scopes.len() * 64)?;
        let root = entry.lexical_scope;
        let keeps_state = |site: &Site, budget: &mut Budget<'_>| -> Result<bool, OwnError> {
            let stack = super::super::edges::stack(module, site_scopes[site])?;
            budget.work(stack.len())?;
            Ok(stack.iter().any(|scope| {
                (*scope == root && self.retained_roots.contains(&function))
                    || self.inventories.get(scope).is_some_and(|facts| {
                        !facts.blockers.is_empty() || !facts.potential_owned_types.is_empty()
                    })
            }))
        };
        // The driver offers a call operand without flow facts (a reference without an obligation) only when it
        // keeps every offer, or at a terminator call, which has no flow snapshot.
        let elides_offers = body.insts.iter().any(|(_, instruction)| {
            matches!(routing::handling(&instruction.op), routing::Handling::Snapshot)
                || matches!(instruction.op, Operation::Call(CallOp::Closure { .. } | CallOp::Cap { .. }
                    | CallOp::Role { .. }))
        }) || declaration.attrs.iter().any(|attr| matches!(attr, FunctionAttr::Lambda { .. }));
        budget.work(body.insts.len())?;
        for (site, op) in operations {
            budget.work(1)?;
            let partitions_inputs = self.call_summary(function, site).is_some_and(|summary| {
                summary.entry_kind != EntryKind::ObservationAdmissionStaysCaller
            });
            // P1.3 §3.4: include failures introduced by elaboration. Offering
            // managed call inputs emits scope.split even for a scalar result.
            // Binding a Future splits nothing: its cache duty stays in the owner state (D12).
            let binds = matches!(op, Operation::Lexical(LexicalOp::Bind { .. } | LexicalOp::Rebind { .. }));
            // Elaboration partitions every value it gives deps (the driver's `managed`): an owned value, a
            // builtin capability (Atomic), and a deps-carrying reference without an obligation (a cap value, a
            // role, a function value), the last only against a kept scope state (#706).
            let partitioned = |owned: bool| cfg::operation_uses(op, body).is_ok_and(|uses| uses.iter().any(|operand| {
                body.values.get(operand.value).is_ok_and(|value| {
                    let facts = &types[&value.ty];
                    let selected = if owned { facts.may_own() || facts.capabilities } else { facts.references };
                    selected && !(binds && futures::is_future(module, value.ty))
                })
            }));
            if module.profile == Profile::High && (partitions_inputs || binds || matches!(op,
                Operation::Lexical(LexicalOp::Stage { .. }) | Operation::Exit(ExitOp::Prepare { .. })))
                // Elaboration partitions an owned value or a builtin capability (Atomic) wherever it gives it
                // deps; a deps-carrying reference without an obligation only against a scope state it keeps
                // (#706). A non-callable reference offered to an owning call or bound, such as a role value with
                // plain implementers or an object holding one, is also counted (#703); an offered `fn` value is
                // left out of that count, as own elides the offer of a closure without deps (`may_offer`).
                && (partitioned(true)
                    || partitioned(false)
                        && !(partitions_inputs && elides_offers && matches!(site, Site::Instruction(_)))
                        && keeps_state(&site, budget)?
                    || (partitions_inputs || binds) && cfg::operation_uses(op, body).is_ok_and(|uses| {
                        uses.iter().any(|operand| body.values.get(operand.value).is_ok_and(|value| {
                            let callable = matches!(module.types.get(value.ty), Ok(Type::Fn(_)));
                            !callable && types[&value.ty].references
                                && !(binds && futures::is_future(module, value.ty))
                        }))
                    })) {
                normal::failures(&mut summary.contract.failures, &[FailureKind::Program]);
            }
            if let Some(callee) = self.call_summary(function, site) {
                normal::failures(&mut summary.contract.failures, &callee.contract.failures);
                summary.contract.suspends |= callee.contract.suspends;
                summary.contract.async_destroy |= callee.contract.async_destroy;
                if let Some((call, receiver)) = call(op) {
                    let arguments: Vec<_> = receiver.iter().chain(&call.args).map(|arg| arg.value).collect();
                    for effect in &callee.parameters {
                        if effect.destination == TransferDestination::None
                            || effect.destination == TransferDestination::Result {
                            continue;
                        }
                        if let Some(selection) = &effect.retention {
                            let relation = instantiate(selection, &arguments, &values, budget)?;
                            retain_inputs((module, types), &mut summary, &relation, effect.destination.clone(), &borrowed, budget)?;
                        }
                    }
                }
            } else if matches!(op, Operation::Ownership(OwnershipOp::End { .. })) {
                // The legacy packet close has no D18 payload. Preserve its
                // declared failure envelope until cleanup specialization gives
                // it a scope contract; sync eligibility still comes from types.
                let contract = ContractRegistry::new(module).construction_schema(op, body)
                    .map_err(|error| budget.fail(OwnDiagnosticKind::InvalidEquation, format!("{error:?}")))?;
                normal::failures(&mut summary.contract.failures, &contract.effects.failures);
            } else if !is_scope_close(op) {
                let contract = ContractRegistry::new(module).high_site_schema(op, body, site.high())
                    .map_err(|error| budget.fail(OwnDiagnosticKind::InvalidEquation, format!("{error:?}")))?;
                normal::failures(&mut summary.contract.failures, &contract.effects.failures);
                summary.contract.suspends |= contract.effects.suspends;
            }
        }
        for (block_id, block) in body.blocks.iter() {
            match &block.term.kind {
                TermKind::RetDefer { tag: DeferReturnTag::Replace(label), values: returned } => {
                    // The tagged ABI's ordinary result is void. Replacement
                    // values still retain inputs at the declared label outcome.
                    for value in returned.iter().filter(|value| body.values.get(**value).is_ok_and(|value| {
                        !language(module, &[value.ty]).is_empty()
                    })) {
                        let relation = replacements.entry(label.clone()).or_insert_with(normal::empty_relation);
                        normal::relation(relation, &values[value.as_u32() as usize], budget)?;
                        retain_inputs((module, types), &mut summary, &values[value.as_u32() as usize],
                            TransferDestination::Result, &borrowed, budget)?;
                    }
                }
                TermKind::Ret { values: returned } | TermKind::RetDefer { values: returned, .. } => {
                    let returned = returned.iter().filter(|value| body.values.get(**value).is_ok_and(|value| {
                        !language(module, &[value.ty]).is_empty()
                    }));
                    for (index, value) in returned.enumerate() {
                        if let Some(result) = summary.results.get_mut(index) {
                            normal::relation(result, &values[value.as_u32() as usize], budget)?;
                        }
                    }
                }
                TermKind::Throw(_) => normal::failures(&mut summary.contract.failures, &[FailureKind::Thrown]),
                TermKind::Panic(_) => normal::failures(&mut summary.contract.failures, &[FailureKind::Panic]),
                TermKind::Trap { .. } => normal::failures(&mut summary.contract.failures, &[FailureKind::Program]),
                // Raise forwards an existing failure. Its sources (operations,
                // throw/panic and accepted failure views) supply the kinds.
                TermKind::Raise(_) => {},
                TermKind::InvokeDefer { .. } => {
                    if let Some(callee) = self.call_summary(function, Site::Terminator(block_id)) {
                        normal::failures(&mut summary.contract.failures, &callee.contract.failures);
                        summary.contract.suspends |= callee.contract.suspends;
                    }
                }
                _ => {}
            }
        }
        if module.profile == Profile::High {
            let mut bindings: BTreeMap<BindingId, ResultRelation> = BTreeMap::new();
            for (_, instruction) in body.insts.iter() {
                if let Operation::Lexical(LexicalOp::Bind { binding, value }
                    | LexicalOp::Rebind { binding, value, .. }) = instruction.op {
                    let relation = bindings.entry(binding).or_insert_with(normal::empty_relation);
                    normal::relation(relation, &values[value.as_u32() as usize], budget)?;
                }
            }
            for (inst, instruction) in body.insts.iter() {
                if let Operation::Lexical(LexicalOp::DeferRegister { action, .. }) = &instruction.op {
                    let mut actuals: Vec<_> = action.captures.iter().map(|value| {
                        values[value.as_u32() as usize].clone()
                    }).collect();
                    for binding in &action.bindings {
                        actuals.push(bindings.get(binding).cloned().unwrap_or_else(normal::empty_relation));
                    }
                    for mapping in &action.replacements {
                        if mapping.kind != ExitKind::Return {
                            continue;
                        }
                        let relation = self.replacement_relation(function, body, Site::Instruction(inst),
                            action.function, &mapping.label, &actuals, budget)?;
                        if let Some(result) = summary.results.first_mut() {
                            normal::relation(result, &relation, budget)?;
                        }
                    }
                }
            }
        }
        // PR-P (#749): a path that goes past a lent field (its own field, reached through a role or a type
        // parameter) does not follow the substituted fields of its parameter's type (T4); it names the lent field
        // whole instead: an over-approximation, so no dependency is lost.
        // An async body's summary describes its Future's outcome, and coro gives the body a frame for parameters
        // (OIR-2 §12): PR-P declarations are a synchronous call boundary's, so an async body keeps none.
        let parameters = language_parameters(module, body);
        let asynchronous = futures::is_async(module, function);
        for result in &mut summary.results {
            let lent: Vec<_> = std::mem::take(&mut result.aliases).into_iter()
                .map(|alias| lent_prefix(module, types, &parameters, alias))
                .filter(|alias| !(asynchronous && carrier_free_projection(module, types, alias))).collect();
            result.aliases = normal::selections(&lent, budget)?;
        }
        if summary.entry_kind == EntryKind::CallBoundary {
            for result in summary.results.clone() {
                retain_inputs((module, types), &mut summary, &result, TransferDestination::Result, &borrowed, budget)?;
            }
        }
        if entry.params.iter().any(|value| body.values.get(*value).is_ok_and(|value| {
            module.types.get(value.ty) == Ok(&Type::Service(ServiceType::Failure))
        })) {
            normal::failures(&mut summary.contract.failures, &[FailureKind::Thrown, FailureKind::Program,
                FailureKind::Panic, FailureKind::Native, FailureKind::Cancelled]);
        }
        for (scope, facts) in &self.inventories {
            if module.scopes.get(*scope).map_err(arena)?.function != function {
                continue;
            }
            for ty in &facts.potential_owned_types {
                let effects = self.cleanup(module, &types[ty]);
                normal::failures(&mut summary.contract.failures, &effects.failures);
                summary.contract.suspends |= effects.suspends;
                summary.contract.async_destroy |= effects.suspends;
            }
            for defer in &facts.defers {
                normal::failures(&mut summary.contract.failures, &self.summary(*defer).contract.failures);
                summary.contract.suspends |= self.summary(*defer).contract.suspends;
            }
            for protocol in &facts.service_protocols {
                let mut effects = ScopeEffectEnvelope::default();
                project(&mut effects, self.protocol_summary(*protocol));
                normal::failures(&mut summary.contract.failures, &effects.failures);
                summary.contract.suspends |= effects.suspends;
                summary.contract.async_destroy |= effects.suspends;
            }
        }
        if declaration.attrs.contains(&FunctionAttr::Async) {
            // The body of an async action may suspend, and its final successful
            // exit delivers deferred cancellation (D18).
            summary.contract.suspends = true;
            normal::failures(&mut summary.contract.failures, &[FailureKind::Cancelled]);
        }
        // Coro gives an async body a frame for parameters (OIR-2 §12), so its retention keeps whole parameters, never
        // a path into one (T4 would type the path against the frame). A field that is not carrier-free, such as a
        // FutureGroup's (#810), is retained through its container: an over-approximation that loses no dependency,
        // as the body's creation summary retains the parameter whole.
        if asynchronous {
            for effect in &mut summary.parameters {
                effect.retention = effect.retention.take().map(whole_parameters);
            }
        }
        normal::normalize(&mut summary, budget)?;
        Ok(BodyInference { summary, replacements })
    }

    #[allow(clippy::too_many_arguments)]
    fn replacement_relation(
        &self,
        function: FuncId,
        body: &Body,
        site: Site,
        callee: FuncId,
        label: &str,
        actuals: &[ResultRelation],
        budget: &mut Budget<'_>,
    ) -> Result<ResultRelation, OwnError> {
        let source = self.replacements.get(&(callee, label.to_owned())).ok_or_else(|| {
            budget.fail(OwnDiagnosticKind::MissingBoundary, "D8 replacement has no local tagged result equation")
        })?;
        let arguments: Vec<_> = (0..actuals.len()).map(|index| ValueId::from_u32(index as u32)).collect();
        let mut result = normal::empty_relation();
        for selection in &source.aliases {
            let selected = instantiate(selection, &arguments, actuals, budget)?;
            normal::relation(&mut result, &selected, budget)?;
        }
        for dep in &source.external_deps {
            if let Some(actual) = actuals.get(usize::from(*dep)) {
                result.external_deps.extend_from_slice(&actual.external_deps);
            }
        }
        if !source.fresh.is_empty() {
            budget.work(body.keys.len() + body.insts.len() + body.blocks.len())?;
            result.fresh.push(site.origin(body, 0, self.strides[&function]));
            result.provenance = source.provenance;
        }
        result.opaque |= source.opaque;
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    fn operation_relations(
        &self,
        module: &Module,
        function: FuncId,
        body: &Body,
        site: Site,
        op: &Operation,
        values: &[ResultRelation],
        types: &BTreeMap<TypeId, TypeClassification>,
        budget: &mut Budget<'_>,
    ) -> Result<Vec<ResultRelation>, OwnError> {
        // A sort's result holds exactly the elements of its array; the comparisons' results stay inside.
        if let Operation::Call(CallOp::Order { array, .. }) = op {
            return Ok(vec![values[array.value.as_u32() as usize].clone()]);
        }
        if let Some((call, receiver)) = call(op) {
            let callee = self.call_summary(function, site).unwrap();
            let arguments: Vec<_> = receiver.iter().chain(&call.args).map(|arg| arg.value).collect();
            if arguments.len() != callee.parameters.len() {
                return Err(budget.fail(OwnDiagnosticKind::IncompatibleSignature, format!(
                    "call {function:?} {site:?}: {} actuals, {} parameters",
                    arguments.len(), callee.parameters.len(),
                )));
            }
            let mut results = vec![];
            for (index, relation) in callee.results.iter().enumerate() {
                let mut result = normal::empty_relation();
                for selection in &relation.aliases {
                    let selected = instantiate(selection, &arguments, values, budget)?;
                    normal::relation(&mut result, &selected, budget)?;
                }
                for dep in &relation.external_deps {
                    if let Some(argument) = arguments.get(usize::from(*dep)) {
                        let actual = &values[argument.as_u32() as usize];
                        result.external_deps.extend_from_slice(&actual.external_deps);
                        // Keep the full dependency positions without turning a
                        // selected field alias into retention of its container.
                        // PR-P (#749): an argument that is a carrier-free field of a parameter is lent by that
                        // field, and a result that depends on it keeps the field's alias.
                        let lent = carrier_free_aliases(module, types, &language_parameters(module, body), actual);
                        normal::relation(&mut result, &lent, budget)?;
                    }
                }
                // Creating a cold Future gives its owner the duty ServiceOf{future, Cache} (D12), a fresh
                // family named the way an identity is (D25.1): a returned Future carries it to the caller. The call
                // of an async intrinsic or native (D22) creates one as an async body's call does (#791 W3).
                let creates = matches!(op, Operation::Call(CallOp::Direct { function: callee, .. }
                    | CallOp::Intrinsic { function: callee, .. } | CallOp::Native { function: callee, .. })
                    if futures::is_async(module, *callee));
                if !relation.fresh.is_empty() || creates {
                    budget.work(body.keys.len() + body.insts.len() + body.blocks.len())?;
                    result.fresh.push(site.origin(body, index, self.strides[&function]));
                    result.provenance = relation.provenance;
                }
                // An opaque callee result stays opaque in every caller relation.
                result.opaque |= relation.opaque;
                results.push(result);
            }
            if call.packet.is_some() {
                results.extend([normal::empty_relation(), normal::empty_relation()]);
            }
            return Ok(results);
        }
        let contract = ContractRegistry::new(module).construction_schema(op, body)
            .map_err(|error| budget.fail(OwnDiagnosticKind::InvalidEquation, format!("{error:?}")))?;
        let mut results = vec![normal::empty_relation(); contract.normal.results.len()];
        for rule in &contract.normal.retention {
            if let Some(index) = contract.normal.results.iter().position(|slot| slot.name == rule.result) {
                let retained = retention(module, body, &rule.selection, values, budget)?;
                normal::relation(&mut results[index], &retained, budget)?;
            }
        }
        match op {
            Operation::Exit(ExitOp::Prepare { exit }) if !results.is_empty() => {
                if let Some(value) = exit.value {
                    results[0] = values[value.as_u32() as usize].clone();
                }
            }
            Operation::Exit(ExitOp::Replace { pending, replacement }) => {
                if let Some(value) = replacement.value {
                    results[0] = values[value.as_u32() as usize].clone();
                }
                if results.len() > 1 {
                    results[1] = values[pending.as_u32() as usize].clone();
                }
            }
            Operation::Exit(ExitOp::Extend { pending, .. }) => {
                results[0] = values[pending.as_u32() as usize].clone();
            }
            Operation::Exit(ExitOp::Commit { pending, result: Some(_), .. }) => {
                results[0] = values[pending.as_u32() as usize].clone();
                results[0].provenance = AdmissionProvenance::UnboundResult;
            }
            Operation::Ownership(OwnershipOp::ScopeEnd { reserve: Some(reserve), .. }) => {
                if let Some(index) = contract.normal.results.iter().position(|slot| slot.name == SlotName::Pending) {
                    results[index] = values[reserve.as_u32() as usize].clone();
                }
            }
            _ => {}
        }
        // A builder transition retains its buffer and every inserted value; its
        // contract carries this as buffer refs rather than a retention formula.
        if let Operation::Collection(collection) = op
            && let Some(buffer) = super::arrays::consumed_buffer(op)
            && let Some(index) = contract.normal.results.iter()
                .position(|slot| matches!(slot.name, SlotName::Buffer | SlotName::Value)) {
            let mut relation = values[buffer.as_u32() as usize].clone();
            for value in super::arrays::inserted(collection) {
                normal::relation(&mut relation, &values[value.as_u32() as usize], budget)?;
            }
            results[index] = relation;
        }
        let created = matches!(op, Operation::Core(CoreOp::ObjNew { construction: ConstructionStatus::Complete, .. }
            | CoreOp::ObjNext { .. } | CoreOp::VariantNew { .. } | CoreOp::FakeNew { .. }));
        let result_types = match site {
            Site::Instruction(inst) => body.insts.get(inst).map_err(arena)?.results.iter().map(|value| {
                body.values.get(*value).map(|value| value.ty).map_err(arena)
            }).collect::<Result<Vec<_>, _>>()?,
            Site::Terminator(block) => {
                let mut result_types = vec![None; results.len()];
                if let TermKind::InvokeOwn { normal, .. } = &body.blocks.get(block).map_err(arena)?.term.kind {
                    for (parameter, argument) in body.blocks.get(normal.block).map_err(arena)?.params.iter()
                        .zip(&normal.args) {
                        if let EdgeArg::Produced(result) = argument {
                            let ty = body.values.get(*parameter).map_err(arena)?.ty;
                            result_types[usize::from(result.slot)] = Some(ty);
                        }
                    }
                }
                // Unused output slots need no SSA relation.
                result_types.into_iter().map(|ty| ty.unwrap_or(TypeId::from_u32(u32::MAX))).collect()
            }
        };
        for (index, (result, ty)) in results.iter_mut().zip(result_types).enumerate() {
            if let Some(facts) = types.get(&ty) {
                // A construction with deps but no obligation keeps the relation to the fields it retains, and is
                // no fresh identity (OIR-2 §3.2; P2.4 §4.8).
                if !facts.may_own() && !(facts.references && created) {
                    // PR-P (#749): a reference without obligation read from a carrier-free field of a parameter
                    // keeps that field as its alias, so a result that keeps it declares the field and its caller
                    // instantiates the field's deps through it. It owns nothing, so nothing else survives.
                    *result = if (facts.references || facts.capabilities) && !created {
                        carrier_free_aliases(module, types, &language_parameters(module, body), result)
                    } else {
                        normal::empty_relation()
                    };
                } else if created && facts.may_own() {
                    budget.work(body.keys.len() + body.insts.len() + body.blocks.len())?;
                    result.fresh.push(site.origin(body, index, self.strides[&function]));
                }
            }
        }
        Ok(results)
    }

}

fn is_scope_close(op: &Operation) -> bool {
    matches!(op, Operation::Ownership(OwnershipOp::ScopeEnd { .. } | OwnershipOp::ScopeUnwind { .. }
        | OwnershipOp::End { .. }) | Operation::Cleanup(_))
}

fn join_value(
    values: &mut [ResultRelation],
    value: ValueId,
    relation: &ResultRelation,
    budget: &mut Budget<'_>,
) -> Result<bool, OwnError> {
    budget.work(1)?;
    let target = &mut values[value.as_u32() as usize];
    let before = target.clone();
    normal::relation(target, relation, budget)?;
    if before != *target {
        budget.memory(128 + target.aliases.len() * 128 + target.fresh.len() * 4)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// The parameters whose graphs `relation` retains become taken, with that retention and destination. A
/// `borrowed` parameter (deps, no obligation) is never taken: the relation's external deps already state that
/// the result depends on it (#700, #731).
fn retain_inputs(
    (module, types): (&Module, &BTreeMap<TypeId, TypeClassification>),
    summary: &mut Summary,
    relation: &ResultRelation,
    destination: TransferDestination,
    borrowed: &BTreeSet<u16>,
    budget: &mut Budget<'_>,
) -> Result<(), OwnError> {
    let mut selections = normal::selections(&relation.aliases, budget)?;
    for parameter in &relation.external_deps {
        if !selections.iter().any(|selection| normal::parameter(selection) == *parameter) {
            selections.push(SummarySelection::OwnerGraph(*parameter));
        }
    }
    // PR-P (#749): a carrier-free field is lent, never taken: the result's alias declares it, and no ownership
    // transfer is recorded for it.
    selections.retain(|selection| !borrowed.contains(&normal::parameter(selection))
        && !carrier_free_projection(module, types, selection));
    for selection in selections {
        let parameter = normal::parameter(&selection);
        let effect = summary.parameters.get_mut(usize::from(parameter)).ok_or_else(|| {
            budget.fail(OwnDiagnosticKind::IncompatibleSignature, "retention index outside boundary")
        })?;
        effect.convention = FreshConvention::Take;
        let mut items: Vec<_> = effect.retention.take().into_iter().collect();
        items.push(selection);
        effect.retention = Some(SummarySelection::Union(normal::selections(&items, budget)?));
        effect.deps.extend_from_slice(&relation.external_deps);
        effect.destination = match (&effect.destination, &destination) {
            (TransferDestination::None, _) => destination.clone(),
            (old, new) if old == new => destination.clone(),
            // A cold Future that retains the input in its cache and is returned takes the
            // input along with its cache duty (OIR-2 §10.1, C-4): the input reaches the result.
            (TransferDestination::Cache, TransferDestination::Result)
            | (TransferDestination::Result, TransferDestination::Cache) => TransferDestination::Result,
            (old, _) => return Err(budget.fail(OwnDiagnosticKind::IncompatibleDestinationJoin, format!(
                "parameter {parameter}: {old:?} conflicts with {destination:?}",
            ))),
        };
    }
    Ok(())
}

/// PR-P (#749): whether a summary selection is a projection (`field`, `path`) whose last field is carrier-free: its
/// type owns nothing and can hold no Future (D25.1). Such a field is lent with its holder's deps; a packet can carry
/// nothing through it.
pub(crate) fn carrier_free_projection(module: &Module, types: &BTreeMap<TypeId, TypeClassification>,
    selection: &SummarySelection) -> bool {
    let last = match selection {
        SummarySelection::Field { field, .. } => *field,
        SummarySelection::Path { fields, .. } => match fields.last() {
            Some(field) => *field,
            None => return false,
        },
        _ => return false,
    };
    let Ok(DeclarationKind::Object { fields, .. }) = module.declarations.get(last.declaration)
        .map(|declaration| &declaration.kind) else {
        return false;
    };
    fields.get(last.index as usize).is_some_and(|field| {
        types.get(&field.ty).is_some_and(|facts| !facts.may_own())
            && !crate::verify::holds_futures(module, field.ty)
    })
}

/// A retention that keeps every parameter it reaches whole (an async body's, see `infer_body`).
fn whole_parameters(selection: SummarySelection) -> SummarySelection {
    match selection {
        SummarySelection::Field { parameter, .. } | SummarySelection::Path { parameter, .. } => {
            SummarySelection::Parameter(parameter)
        }
        SummarySelection::Union(items) => SummarySelection::Union(items.into_iter().map(whole_parameters).collect()),
        other => other,
    }
}

/// PR-P (#749): the carrier-free projections of `relation`, alone: no fresh family, no external deps. A projection
/// is kept only along the substituted fields of its parameter's type (T4): a path that crossed a role, a type
/// parameter or another value's fields does not name a field of the parameter.
fn carrier_free_aliases(module: &Module, types: &BTreeMap<TypeId, TypeClassification>, parameters: &[TypeId],
    relation: &ResultRelation) -> ResultRelation {
    let mut kept = normal::empty_relation();
    kept.aliases = relation.aliases.iter().filter(|selection| carrier_free_projection(module, types, selection)
        && lent_path(module, parameters, selection)).cloned().collect();
    kept
}

/// Whether a `field`/`path` selection follows the substituted object fields of its parameter's type (T4).
fn lent_path(module: &Module, parameters: &[TypeId], selection: &SummarySelection) -> bool {
    let (parameter, fields) = match selection {
        SummarySelection::Field { parameter, field } => (*parameter, std::slice::from_ref(field)),
        SummarySelection::Path { parameter, fields } => (*parameter, fields.as_slice()),
        _ => return false,
    };
    parameters.get(usize::from(parameter))
        .is_some_and(|ty| crate::verify::summary_path_follows(module, *ty, fields))
}

/// PR-P (#749): a projection that goes past a lent field of its parameter becomes that field: the longest prefix that
/// follows the parameter's substituted fields (T4), when it ends at a carrier-free field. Any other selection is kept.
fn lent_prefix(module: &Module, types: &BTreeMap<TypeId, TypeClassification>, parameters: &[TypeId],
    selection: SummarySelection) -> SummarySelection {
    let SummarySelection::Path { parameter, fields } = &selection else {
        return selection;
    };
    let Some(&ty) = parameters.get(usize::from(*parameter)) else {
        return selection;
    };
    if crate::verify::summary_path_follows(module, ty, fields) {
        return selection;
    }
    let prefix = (1..fields.len()).rev()
        .find(|length| crate::verify::summary_path_follows(module, ty, &fields[..*length]))
        .map(|length| match length {
            1 => SummarySelection::Field { parameter: *parameter, field: fields[0] },
            _ => SummarySelection::Path { parameter: *parameter, fields: fields[..length].to_vec() },
        });
    match prefix {
        Some(prefix) if carrier_free_projection(module, types, &prefix) => prefix,
        _ => selection,
    }
}

/// The types of a body's language parameters, in summary order.
fn language_parameters(module: &Module, body: &Body) -> Vec<TypeId> {
    body.blocks.get(body.entry).map(|entry| entry.params.iter().filter_map(|value| {
        let ty = body.values.get(*value).ok()?.ty;
        (!language(module, &[ty]).is_empty()).then_some(ty)
    }).collect()).unwrap_or_default()
}

fn project_relation(
    relation: &ResultRelation,
    field: Option<FieldRef>,
    owner_graph: bool,
    budget: &mut Budget<'_>,
) -> Result<ResultRelation, OwnError> {
    let mut result = relation.clone();
    if field.is_some() {
        // A field selection retains its own graph. Whole-parameter external
        // deps would also retain the container's identity at an escape.
        result.external_deps.clear();
    }
    result.aliases.clear();
    for selection in normal::selections(&relation.aliases, budget)? {
        let parameter = normal::parameter(&selection);
        let mut fields = match selection {
            SummarySelection::Field { field, .. } => vec![field],
            SummarySelection::Path { fields, .. } => fields,
            SummarySelection::OwnerGraph(_) | SummarySelection::Descriptor(_) => {
                result.aliases.push(SummarySelection::OwnerGraph(parameter));
                continue;
            }
            _ => vec![],
        };
        if let Some(field) = field {
            fields.push(field);
        }
        result.aliases.push(if owner_graph || fields.len() > 4 {
            SummarySelection::OwnerGraph(parameter)
        } else if fields.is_empty() {
            SummarySelection::Parameter(parameter)
        } else {
            SummarySelection::Path { parameter, fields }
        });
    }
    Ok(result)
}

fn instantiate(
    selection: &SummarySelection,
    arguments: &[ValueId],
    values: &[ResultRelation],
    budget: &mut Budget<'_>,
) -> Result<ResultRelation, OwnError> {
    let mut result = normal::empty_relation();
    for selection in normal::selections(std::slice::from_ref(selection), budget)? {
        let parameter = normal::parameter(&selection);
        let value = arguments.get(usize::from(parameter)).ok_or_else(|| {
            budget.fail(OwnDiagnosticKind::IncompatibleSignature, "selection index outside call arguments")
        })?;
        let mut actual = values[value.as_u32() as usize].clone();
        match selection {
            SummarySelection::Field { field, .. } => {
                actual = project_relation(&actual, Some(field), false, budget)?;
            }
            SummarySelection::Path { fields, .. } => {
                for field in fields {
                    actual = project_relation(&actual, Some(field), false, budget)?;
                }
            }
            SummarySelection::OwnerGraph(_) | SummarySelection::Descriptor(_) => {
                actual = project_relation(&actual, None, true, budget)?;
            }
            _ => {}
        }
        normal::relation(&mut result, &actual, budget)?;
    }
    Ok(result)
}

fn retention(
    module: &Module,
    body: &Body,
    retention: &Retention,
    values: &[ResultRelation],
    budget: &mut Budget<'_>,
) -> Result<ResultRelation, OwnError> {
    let mut field = None;
    let mut owner = false;
    let inputs: Vec<_> = match retention {
        Retention::Global(_) => vec![],
        Retention::FullDeps(value) | Retention::Value(value) | Retention::Index { base: value, .. }
        | Retention::Slice { base: value, .. } | Retention::Keys(value) | Retention::Values(value)
        | Retention::MapEntry { map: value, .. } | Retention::MapPosition { map: value, .. }
        | Retention::ArrayRemove { array: value, .. } | Retention::Difference { base: value, .. }
        | Retention::Cache(value) | Retention::TakenOutcome { future: value, .. }
        | Retention::AwaitFailure { future: value, .. } => vec![*value],
        Retention::Field { base, field: selected } => {
            field = Some(*selected);
            vec![*base]
        }
        Retention::OwnerGraph(value) | Retention::ViewOwner(value)
        | Retention::Descriptor { value, .. } => {
            owner = true;
            vec![*value]
        }
        Retention::ArrayReplace { array, value, .. } => vec![*array, *value],
        Retention::MapMerge(inputs) | Retention::Union(inputs) | Retention::CarrierGraph(inputs) => inputs.clone(),
        Retention::ObjectFields { object, updates } => {
            let ty = body.values.get(*object).map_err(arena)?.ty;
            let Type::Object { decl, .. } = module.types.get(ty).map_err(arena)? else {
                return Err(flow_error("successor receiver is not an object"));
            };
            let DeclarationKind::Object { fields, .. } = &module.declarations.get(*decl).map_err(arena)?.kind else {
                return Err(flow_error("successor receiver has no field layout"));
            };
            let mut result = normal::empty_relation();
            for index in 0..fields.len() {
                let field = FieldRef { declaration: *decl, index: index as u32 };
                let actual = if let Some(update) = updates.iter().find(|update| update.field == field) {
                    values[update.value.as_u32() as usize].clone()
                } else {
                    project_relation(&values[object.as_u32() as usize], Some(field), false, budget)?
                };
                normal::relation(&mut result, &actual, budget)?;
            }
            return Ok(result);
        }
        Retention::Entries(entries) => entries.iter().flat_map(|(key, value)| [*key, *value]).collect(),
    };
    let mut result = normal::empty_relation();
    for input in inputs {
        let actual = project_relation(&values[input.as_u32() as usize], field, owner, budget)?;
        normal::relation(&mut result, &actual, budget)?;
    }
    Ok(result)
}
