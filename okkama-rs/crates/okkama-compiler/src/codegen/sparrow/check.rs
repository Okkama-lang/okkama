//! Независимая проверка распределения регистров (H-10): `registers` красит
//! значения IR по живости, а ядро Sparrow доверяет результату (по нему
//! строятся его стековые карты). Проверка не повторяет анализ живости
//! распределителя: она идёт по исходной функции и для каждого регистра
//! помнит значение, которое в нём наверняка лежит (must-анализ, как checker
//! regalloc2). Чтение требует своё значение в своём регистре на всех путях;
//! переход пишет параметры цели одновременно, после чтения всех аргументов;
//! обработчик unwind получает состояние, общее для всех точек блока, где
//! возможен отказ.
//!
//! Запись, которую машинный код может сделать раньше чтения операндов,
//! проверяется отдельно (правило распределителя, `registers`): результат
//! инструкции и значение ребра (найденное значение lookup, пойманное значение
//! `catch`: параметры цели сверх аргументов перехода) не лежат в регистре
//! операнда той же инструкции или терминатора (#1007, #1043).
use crate::ir::{Function, ValueId};

type State = Vec<Option<usize>>;

/// Регистр каждого значения: функции одной формы, значения переименованы.
fn colors(original: &Function, allocated: &Function) -> Result<Vec<usize>, String> {
    let mut color = vec![usize::MAX; original.values.len()];
    let mut assign = |value: &ValueId, register: &ValueId| -> Result<(), String> {
        match color[value.0] {
            usize::MAX => {
                color[value.0] = register.0;
                Ok(())
            }
            known if known == register.0 => Ok(()),
            known => Err(format!("v{} is renamed to both r{known} and r{}", value.0, register.0)),
        }
    };
    if original.blocks.len() != allocated.blocks.len() || original.params.len() != allocated.params.len() {
        return Err("the allocated function has another shape".into());
    }
    for (value, register) in original.params.iter().zip(&allocated.params) {
        assign(value, register)?;
    }
    for (block, renamed) in original.blocks.iter().zip(&allocated.blocks) {
        if block.insts.len() != renamed.insts.len() || block.params.len() != renamed.params.len() {
            return Err("the allocated function has another shape".into());
        }
        for (value, register) in block.params.iter().zip(&renamed.params) {
            assign(value, register)?;
        }
        for (inst, renamed) in block.insts.iter().zip(&renamed.insts) {
            if let (Some(value), Some(register)) = (inst.result, renamed.result) {
                assign(&value, &register)?;
            }
        }
    }
    // Every use is renamed consistently with its definition.
    for (block, renamed) in original.blocks.iter().zip(&allocated.blocks) {
        let uses = block.insts.iter().flat_map(|inst| inst.kind.operands()).chain(block.term.kind.operands());
        let renamed_uses = renamed.insts.iter().flat_map(|inst| inst.kind.operands()).chain(renamed.term.kind.operands());
        for (value, register) in uses.zip(renamed_uses) {
            if color[value.0] != register.0 {
                return Err(format!("a use of v{} reads r{}, its definition writes r{}", value.0, register.0, color[value.0]));
            }
        }
        let targets = block.term.kind.targets().into_iter().chain(block.unwind.as_ref());
        let renamed_targets = renamed.term.kind.targets().into_iter().chain(renamed.unwind.as_ref());
        for (target, renamed) in targets.zip(renamed_targets) {
            for (value, register) in target.args.iter().zip(&renamed.args) {
                if color[value.0] != register.0 {
                    return Err(format!("an edge argument v{} reads r{}, its definition writes r{}", value.0, register.0, color[value.0]));
                }
            }
        }
    }
    Ok(color)
}

/// The registers of `allocated` hold, at every read, the value `original`
/// reads there.
pub(super) fn check(original: &Function, allocated: &Function) -> Result<(), String> {
    if original.blocks.is_empty() {
        return Ok(());
    }
    let color = colors(original, allocated)?;
    let registers = allocated.values.len();
    let mut entry: Vec<Option<State>> = vec![None; original.blocks.len()];
    let mut start = vec![None; registers];
    for param in original.params.iter().chain(&original.blocks[0].params) {
        start[color[param.0]] = Some(param.0);
    }
    entry[0] = Some(start);
    let mut work = vec![0usize];
    // Join an incoming state into a block's entry: a register keeps a value
    // only when every incoming path agrees on it.
    let join = |entry: &mut Vec<Option<State>>, work: &mut Vec<usize>, block: usize, incoming: State| {
        let changed = match &mut entry[block] {
            None => {
                entry[block] = Some(incoming);
                true
            }
            Some(existing) => {
                let mut changed = false;
                for (held, new) in existing.iter_mut().zip(&incoming) {
                    if held.is_some() && held != new {
                        *held = None;
                        changed = true;
                    }
                }
                changed
            }
        };
        if changed && !work.contains(&block) {
            work.push(block);
        }
    };
    let read = |state: &State, value: ValueId, what: &str| -> Result<(), String> {
        let register = color[value.0];
        match state[register] {
            Some(held) if held == value.0 => Ok(()),
            Some(held) => Err(format!("{what} reads v{} from r{register}, which holds v{held}", value.0)),
            None => Err(format!("{what} reads v{} from r{register}, which does not hold it on every path", value.0)),
        }
    };
    // A write machine code may make before it reads the operands: never into an operand's register.
    let early_write = |value: ValueId, operands: &[ValueId], what: &str| -> Result<(), String> {
        match operands.iter().find(|operand| color[operand.0] == color[value.0]) {
            Some(operand) => Err(format!("{what} writes v{} into r{}, the register of its operand v{}", value.0, color[value.0], operand.0)),
            None => Ok(()),
        }
    };
    // Arguments are read first, then every parameter is written at once.
    let edge = |state: &State, target: &crate::ir::Target, what: &str| -> Result<State, String> {
        for arg in &target.args {
            read(state, *arg, what)?;
        }
        let mut next = state.clone();
        for param in &original.blocks[target.block.0].params {
            next[color[param.0]] = Some(param.0);
        }
        Ok(next)
    };
    while let Some(index) = work.pop() {
        let block = &original.blocks[index];
        let mut state = entry[index].clone().expect("reached");
        let at = |point: usize| format!("block {index}, instruction {point}");
        for (point, inst) in block.insts.iter().enumerate() {
            // A failure here leaves this state for the unwind handler.
            if let Some(unwind) = &block.unwind {
                let next = edge(&state, unwind, &format!("the unwind edge at {}", at(point)))?;
                join(&mut entry, &mut work, unwind.block.0, next);
            }
            let operands = inst.kind.operands();
            for &operand in &operands {
                read(&state, operand, &at(point))?;
            }
            if let Some(result) = inst.result {
                early_write(result, &operands, &at(point))?;
                state[color[result.0]] = Some(result.0);
            }
        }
        if let Some(unwind) = &block.unwind {
            let next = edge(&state, unwind, &format!("the unwind edge at the terminator of block {index}"))?;
            join(&mut entry, &mut work, unwind.block.0, next);
        }
        let operands = block.term.kind.operands();
        for &operand in &operands {
            read(&state, operand, &format!("the terminator of block {index}"))?;
        }
        for target in block.term.kind.targets() {
            // The terminator itself writes the parameters beyond the edge's arguments (an edge value).
            for param in original.blocks[target.block.0].params.iter().skip(target.args.len()) {
                early_write(*param, &operands, &format!("the edge value of block {index}'s terminator"))?;
            }
            let next = edge(&state, target, &format!("the edge from block {index} to block {}", target.block.0))?;
            join(&mut entry, &mut work, target.block.0, next);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{self, ValueId};
    use crate::{SourceFile, lowering::lower_program, syntax};

    fn module(source: &str) -> ir::Module {
        let source = SourceFile::new("test.ok", source).unwrap();
        let ast = syntax::parse(&source, &syntax::ParseOptions::default()).unwrap();
        crate::lowering::with_passes("all", || lower_program(&ast)).unwrap()
    }

    fn function<'a>(module: &'a ir::Module, symbol: &str) -> &'a Function {
        module.functions.iter().find(|function| function.symbol == symbol).expect(symbol)
    }

    /// `function` with every value renamed through `map` (an "allocation").
    fn renamed(function: &Function, map: impl Fn(ValueId) -> ValueId) -> Function {
        let rename = |value: &mut ValueId| *value = map(*value);
        let mut out = function.clone();
        out.params.iter_mut().for_each(rename);
        for block in &mut out.blocks {
            block.params.iter_mut().for_each(rename);
            for inst in &mut block.insts {
                inst.result.iter_mut().for_each(rename);
                inst.kind.operands_mut().into_iter().for_each(rename);
            }
            block.term.kind.operands_mut().into_iter().for_each(rename);
            for target in block.term.kind.targets_mut().into_iter().chain(block.unwind.as_mut()) {
                target.args.iter_mut().for_each(rename);
            }
        }
        out
    }

    /// Values of `function` live across a later definition: `(live, defined)`.
    fn overlapping(function: &Function) -> (ValueId, ValueId) {
        for block in &function.blocks {
            for (index, inst) in block.insts.iter().enumerate() {
                let Some(defined) = inst.result else { continue };
                for later in &block.insts[index + 1..] {
                    for operand in later.kind.operands() {
                        let earlier = block.insts[..index].iter().any(|inst| inst.result == Some(operand));
                        if earlier && function.values[operand.0] == function.values[defined.0] {
                            return (operand, defined);
                        }
                    }
                }
            }
        }
        panic!("no overlapping values")
    }

    #[test]
    fn the_allocator_passes_and_each_corruption_is_caught() {
        let module = module(
            "module app;
            enum Boom is Error = (boom(code is int));
            object Work {
                Work();
                mix(x is int, y is int) is int { let a = x * 3; let b = y * 5; let c = a * b; a + b + c; }
                fail(n is int) is int { if n > 2 { throw Boom.boom(n); } n; }
                guarded(x is int, y is int) is int {
                    let kept = x * 7;
                    let result = try { self.fail(y) * 2; } catch Boom.boom(code) { kept + code; };
                    result + kept;
                }
                pick(flag is bool, x is int, y is int) is int { let p = if flag { x; } else { y; }; let q = if flag { y; } else { x; }; p - q; }
                count(n is int, total is int) is int { if n == 0 { total; } else { self.count(n - 1, total + n * 2); } }
            }
            main() is int { let w = Work(); w.mix(1, 2) + w.guarded(1, 5) + w.pick(true, 3, 4) + w.count(9, 0); }",
        );
        for function in &module.functions {
            let identity = renamed(function, |value| value);
            check(function, &identity).unwrap_or_else(|error| panic!("{}: {error}", function.symbol));
            check(function, &super::super::registers::allocate(function)).unwrap_or_else(|error| panic!("{}: {error}", function.symbol));
        }
        // A value overwritten while it is still needed.
        let mix = function(&module, "Work.mix");
        let (live, defined) = overlapping(mix);
        let merged = renamed(mix, |value| if value == defined { live } else { value });
        assert!(check(mix, &merged).unwrap_err().contains(&format!("which holds v{}", defined.0)));
        // A value the unwind handler needs, overwritten inside the protected block.
        let guarded = function(&module, "Work.guarded");
        let (needed, block) = guarded
            .blocks
            .iter()
            .find_map(|block| block.unwind.as_ref().and_then(|unwind| unwind.args.first().map(|arg| (*arg, block))))
            .expect("an unwind argument");
        // The check follows registers, not types.
        let inside = block.insts.iter().find_map(|inst| inst.result).expect("a value defined in the protected block");
        let clobbered = renamed(guarded, |value| if value == inside { needed } else { value });
        assert!(check(guarded, &clobbered).unwrap_err().contains("unwind edge"));
        // Block parameters written in the wrong order (the loop of a tail call).
        let count = function(&module, "Work.count");
        let mut swapped = renamed(count, |value| value);
        let join = swapped.blocks.iter_mut().find(|block| block.params.len() >= 2).expect("a block with two parameters");
        join.params.swap(0, 1);
        assert!(check(count, &swapped).is_err());
    }

    /// Uses of each value in `function`: operands, terminator operands and edge arguments.
    fn uses(function: &Function) -> Vec<usize> {
        let mut uses = vec![0; function.values.len()];
        for block in &function.blocks {
            let operands = block.insts.iter().flat_map(|inst| inst.kind.operands()).chain(block.term.kind.operands());
            let args = block.term.kind.targets().into_iter().chain(block.unwind.as_ref()).flat_map(|target| target.args.clone());
            for value in operands.chain(args) {
                uses[value.0] += 1;
            }
        }
        uses
    }

    /// A terminator's edge value (the found value of a lookup, the value `catch` caught) in the register of one of
    /// its operands: the must-analysis alone accepts it, since nothing reads that operand after the edge, and the
    /// early-write rule refuses it (#1043), as `registers` never makes it (#1007).
    #[test]
    fn an_edge_value_in_an_operand_register_is_refused() {
        let module = module(
            "module app;
            enum Boom is Error = (boom(code is int));
            object Calc {
                Calc();
                pick(xs is @int, i is int) is int { xs[i] else -1; }
                find(m is %int:int, k is int) is int { m[k] else -1; }
                fail(n is int) is int { if n > 2 { throw Boom.boom(n); } n; }
                guarded(y is int) is int { try { self.fail(y); } catch Boom.boom(code) { code; }; }
            }
            main() is int { let c = Calc(); c.pick([5, 6], 1) + c.find([1: 2], 1) + c.guarded(5); }",
        );
        let mut seen = 0;
        for symbol in ["Calc.pick", "Calc.find", "Calc.guarded"] {
            let function = function(&module, symbol);
            check(function, &super::super::registers::allocate(function)).unwrap_or_else(|error| panic!("{symbol}: {error}"));
            for block in &function.blocks {
                let payload = match &block.term.kind {
                    ir::TermKind::ArrayLookup { found, .. } | ir::TermKind::MapLookup { found, .. } => found,
                    ir::TermKind::Catch { handler, .. } => handler,
                    _ => continue,
                };
                let value = *function.blocks[payload.block.0].params.last().expect("an edge value");
                // The index, the key or the failure: the target's parameters replace it, so nothing reads it later.
                let operand = *block.term.kind.operands().last().expect("an operand");
                let shared = renamed(function, |v| if v == value { operand } else { v });
                let error = check(function, &shared).expect_err(symbol);
                assert!(error.contains("edge value") && error.contains(&format!("operand v{}", operand.0)), "{symbol}: {error}");
                seen += 1;
            }
        }
        assert_eq!(seen, 3);
    }

    /// An instruction's result in the register of an operand it reads last: the same early-write rule.
    #[test]
    fn a_result_in_its_operand_register_is_refused() {
        let module = module(
            "module app;
            object Work { Work(); mix(x is int, y is int) is int { let a = x * 3; let b = y * 5; a + b + a * b; } }
            main() is int { Work().mix(1, 2); }",
        );
        let mix = function(&module, "Work.mix");
        let uses = uses(mix);
        let (operand, result) = mix.blocks.iter().flat_map(|block| &block.insts).find_map(|inst| {
            let result = inst.result?;
            let operand = inst.kind.operands().into_iter()
                .find(|operand| uses[operand.0] == 1 && mix.values[operand.0] == mix.values[result.0]
                    && !mix.params.contains(operand))?;
            Some((operand, result))
        }).expect("an operand read last by an instruction of its type");
        let shared = renamed(mix, |v| if v == result { operand } else { v });
        let error = check(mix, &shared).expect_err("a result in its operand's register");
        assert!(error.contains(&format!("writes v{} into r{}, the register of its operand v{}", result.0, operand.0, operand.0)), "{error}");
    }
}
