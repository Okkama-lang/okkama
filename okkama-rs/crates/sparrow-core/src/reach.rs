//! The functions a run of an SPB program can reach (#1135): what the playground warns about is what a run can call,
//! not everything the imported modules declare (std `fs/file` declares more than twenty natives).
//!
//! Over-approximation over the program's tables, so a native a run calls is always reached: the roots are the
//! entry and the tests of a test profile; a reached function reaches the functions it calls directly or makes a
//! closure of, the init of each constant it reads, the field function of each cap field it calls, its intrinsic's
//! helpers, and each object type it builds (`obj.new`, `variant.new`, an intrinsic's objects); the runtime's own
//! objects (`builtin`) are built from the start. A built object type reaches its destroy, and its implementation of
//! a role method once that method can be called on it: a reached `call.role` of that role and slot, or a call the
//! runtime makes itself ([`RUNTIME_CALLS`]: the error text of an uncaught failure, and the `size`, `item` and `order`
//! methods intrinsics call). A function never named that way can run only through one of these, so it is not
//! reached.
use okkama_sparrow::{FuncRef, Instruction as I, ObjectRef, Program};
use std::collections::HashSet;

/// Role methods the runtime calls by name, not through a `call.role` of the program: (role, method), `None` any
/// role. `Error.text` writes an uncaught failure's report (`failures.rs`); `size` and `item` read a sequence, `order`
/// sorts (`intrinsics.rs`, `drivers.rs`), only from an intrinsic. `the_runtime_calls_are_listed` counts the places.
pub const RUNTIME_CALLS: &[(Option<&str>, &str, bool)] =
    &[(Some("Error"), "text", false), (None, "size", true), (None, "item", true), (None, "order", true)];

/// For each function of `program`: can a run reach it?
pub fn functions(program: &Program) -> Vec<bool> {
    let mut walk = Walk {
        program,
        functions: vec![false; program.functions.len()],
        objects: vec![false; program.objects.len()],
        pending: Vec::new(),
        called: HashSet::new(),
        intrinsics: false,
    };
    if let Some(entry) = program.entry {
        walk.function(entry);
    }
    for (_, test) in program.tests.iter().flatten() {
        walk.function(*test);
    }
    for (index, object) in program.objects.iter().enumerate() {
        if object.builtin {
            walk.object(ObjectRef(index as u32));
        }
    }
    loop {
        while let Some(index) = walk.pending.pop() {
            walk.code(index);
        }
        // Implementations of the methods that can now be called on the objects built so far.
        let before = walk.functions.iter().filter(|reached| **reached).count();
        for (index, object) in program.objects.iter().enumerate() {
            if !walk.objects[index] {
                continue;
            }
            for witness in &object.does {
                for (slot, &implementation) in witness.slots.iter().enumerate() {
                    if walk.callable(witness.role.0, slot as u32) {
                        walk.function(implementation);
                    }
                }
            }
        }
        if walk.functions.iter().filter(|reached| **reached).count() == before {
            return walk.functions;
        }
    }
}

struct Walk<'a> {
    program: &'a Program,
    functions: Vec<bool>,
    objects: Vec<bool>,
    pending: Vec<usize>,
    /// (role, slot) of every reached `call.role`.
    called: HashSet<(u32, u32)>,
    /// Is an intrinsic reached (it may call `size`, `item`, `order`)?
    intrinsics: bool,
}

impl Walk<'_> {
    fn function(&mut self, function: FuncRef) {
        let index = function.0 as usize;
        if let Some(reached) = self.functions.get_mut(index)
            && !*reached
        {
            *reached = true;
            self.pending.push(index);
        }
    }

    fn object(&mut self, object: ObjectRef) {
        let index = object.0 as usize;
        let Some(reached) = self.objects.get_mut(index) else { return };
        if !std::mem::replace(reached, true)
            && let Some(destroy) = self.program.objects[index].destroy
        {
            self.function(destroy);
        }
    }

    fn callable(&self, role: u32, slot: u32) -> bool {
        if self.called.contains(&(role, slot)) {
            return true;
        }
        let Some(declared) = self.program.roles.get(role as usize) else { return false };
        let Some(method) = declared.methods.get(slot as usize) else { return false };
        RUNTIME_CALLS.iter().any(|&(name, called, by_intrinsic)| {
            method.name == called
                && name.is_none_or(|name| declared.builtin && declared.name == name)
                && (!by_intrinsic || self.intrinsics)
        })
    }

    fn code(&mut self, index: usize) {
        let program = self.program;
        let function = &program.functions[index];
        if let Some(intrinsic) = &function.intrinsic {
            self.intrinsics = true;
            intrinsic.helpers.iter().for_each(|&helper| self.function(helper));
            intrinsic.objects.iter().for_each(|&object| self.object(object));
        }
        for instruction in &function.code {
            match instruction {
                I::Call { function, .. } | I::Closure { function, .. } => self.function(*function),
                I::GlobalGet { global, .. } => {
                    if let Some(global) = program.globals.get(*global as usize) {
                        self.function(global.init);
                    }
                }
                I::ObjNew { object, .. } => self.object(*object),
                I::VariantNew { variant, case, .. } => {
                    if let Some((_, object)) = program.variants.get(variant.0 as usize).and_then(|v| v.cases.get(*case as usize)) {
                        self.object(*object);
                    }
                }
                I::CallCap { ty, field, .. } => {
                    if let Some((_, init)) = program.caps.get(ty.0 as usize).and_then(|cap| cap.fields.get(*field as usize)) {
                        self.function(*init);
                    }
                }
                I::CallRole { role, slot, .. } => {
                    self.called.insert((role.0, *slot));
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(text: &str) -> Program {
        okkama_sparrow::assemble(text).unwrap_or_else(|error| panic!("{error}\n{text}"))
    }

    const HEAD: &str = r#"sparrow 18
module "reach" "1.0.0"
type t0 int
type t1 str
type t2 role role0
type t3 object obj0
type t4 object obj1
role role0 "Shape" {
    method "area" [] -> t0
}
object obj0 "Square" {
    does role0 [f2]
}
object obj1 "Unused" {
    does role0 [f3]
    destroy f4
}
function f0 "host" native "runtime/system@1::system_host_name/0" {
    registers []
    params []
    result t1
}
function f1 "never" native "runtime/fs@1::path_parent/2" {
    registers [t1, t0]
    params [r0, r1]
    result t1
}
function f2 "Square.area" {
    registers [t3, t1, t0]
    params [r0]
    result t0
    0: call r1, f0, []
    1: const r2, int 4
    2: ret r2
}
function f3 "Unused.area" {
    registers [t4, t1, t0]
    params [r0]
    result t0
    0: const r1, str "x"
    1: const r2, int 0
    2: call r1, f1, [r1, r2]
    3: ret r2
}
function f4 "Unused.destroy" {
    registers [t4]
    params [r0]
    result void
    0: ret _
}
function f5 "main" {
    registers [t3, t2, t0]
    params []
    result t0
    0: obj.new r0, obj0, []
    1: role.wrap r1, r0, role0
    2: call.role r2, r1, role0, 0, []
    3: ret r2
}
entry f5
"#;

    /// A role call reaches the implementations of the objects the program builds, not of every object declared.
    #[test]
    fn only_built_objects_lend_their_implementations() {
        let reached = functions(&program(HEAD));
        assert_eq!(reached, vec![true, false, true, false, false, true]);
    }

    /// Without an entry or tests nothing runs; the tests of a test profile are roots.
    #[test]
    fn the_roots_are_the_entry_and_the_tests() {
        let mut library = program(HEAD);
        library.entry = None;
        assert!(functions(&library).iter().all(|reached| !reached));
        library.tests = Some(vec![("unused".to_string(), FuncRef(3))]);
        let reached = functions(&library);
        assert_eq!(reached, vec![false, true, false, true, false, false]);
    }

    /// #1135: a built object lends an implementation only to a method that can be called: here both objects are
    /// built but no role method is called, so neither `area` nor the natives they call are reached; `Unused`'s
    /// destroy is.
    #[test]
    fn an_implementation_needs_a_call_of_its_method() {
        let text = HEAD
            .replace("    registers [t3, t2, t0]\n", "    registers [t3, t2, t0, t4]\n")
            .replace("    1: role.wrap r1, r0, role0\n    2: call.role r2, r1, role0, 0, []\n    3: ret r2", "    1: obj.new r3, obj1, []\n    2: const r2, int 0\n    3: ret r2");
        assert_eq!(functions(&program(&text)), vec![false, false, false, false, true, true]);
    }

    /// The runtime calls role methods by name in these places only (program.rs and jit.rs build `call.role`'s
    /// dispatch tables); a new one must join [`RUNTIME_CALLS`], or a method it calls would count as unreachable.
    #[test]
    fn the_runtime_calls_are_listed() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut places = Vec::new();
        let mut stack = vec![dir.clone()];
        while let Some(path) = stack.pop() {
            for entry in std::fs::read_dir(&path).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|extension| extension == "rs") && !path.ends_with("reach.rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    let count = text.matches(".slots[").count() + text.matches("call_role(ctx").count();
                    if count > 0 {
                        places.push((path.strip_prefix(&dir).unwrap().display().to_string(), count));
                    }
                }
            }
        }
        places.sort();
        let expected: Vec<(String, usize)> =
            [("drivers.rs", 2), ("failures.rs", 1), ("intrinsics.rs", 5), ("jit.rs", 1), ("program.rs", 2)].iter().map(|(file, n)| (file.to_string(), *n)).collect();
        assert_eq!(places, expected, "role methods called by the runtime: update RUNTIME_CALLS and this list");
    }


    /// The runtime calls `Error.text` itself (an uncaught failure's report); `size` it calls only from an
    /// intrinsic, and this program has none.
    #[test]
    fn the_runtime_reaches_the_error_text_by_itself() {
        let text = r#"sparrow 18
module "reach" "1.0.0"
type t0 int
type t1 str
type t2 object obj0
role role0 "Error" builtin {
    method "text" [] -> t1
}
role role1 "Sized" {
    method "size" [] -> t0
}
object obj0 "Oops" {
    does role0 [f0]
    does role1 [f1]
}
function f0 "Oops.text" {
    registers [t2, t1]
    params [r0]
    result t1
    0: const r1, str "oops"
    1: ret r1
}
function f1 "Oops.size" {
    registers [t2, t0]
    params [r0]
    result t0
    0: const r1, int 1
    1: ret r1
}
function f2 "main" {
    registers [t2, t0]
    params []
    result t0
    0: obj.new r0, obj0, []
    1: const r1, int 0
    2: ret r1
}
entry f2
"#;
        assert_eq!(functions(&program(text)), vec![true, false, true]);
    }

}
