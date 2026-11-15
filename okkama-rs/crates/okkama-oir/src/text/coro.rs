//! Text forms of the async ABI (D20–D25): publication outcomes, rehome
//! operand forms, service entries, schedule duties and the frame layout.
use super::*;

impl Reader<'_> {
    /// True when the next operand is another positional SSA value: `, %v`.
    fn operand_value(&self) -> bool {
        let mut pos = self.pos;
        let newline = |pos: usize| self.tokens.get(pos).is_some_and(|t| t.text == "\n" && !t.quoted);
        while newline(pos) {
            pos += 1;
        }
        if !self.tokens.get(pos).is_some_and(|t| t.text == "," && !t.quoted) {
            return false;
        }
        pos += 1;
        while newline(pos) {
            pos += 1;
        }
        self.tokens.get(pos).is_some_and(|t| t.text.starts_with('%') && !t.quoted)
    }

    /// `frame.layout { slot N: T [linear] ... }` followed by its continuation lines.
    pub(super) fn frame_layout(&mut self) -> Result<FrameLayout, Diagnostic> {
        self.expect("frame.layout")?;
        self.expect("{")?;
        let mut layout = FrameLayout::default();
        while !self.eat("}") {
            self.entity()?;
            self.expect("slot")?;
            let index = u32::read(self)?;
            if index as usize != layout.slots.len() {
                return Err(self.error(DiagnosticKind::Structure, "frame slots must be numbered in order"));
            }
            self.expect(":")?;
            let ty = TypeId::read(self)?;
            let linear = self.eat("linear");
            layout.slots.push(FrameSlotDecl { ty, linear });
        }
        self.line()?;
        while self.is("continuation") {
            self.entity()?;
            self.expect("continuation")?;
            let index = u32::read(self)?;
            if index as usize != layout.continuations.len() {
                return Err(self.error(DiagnosticKind::Structure, "continuations must be numbered in order"));
            }
            let site = if self.eat("start") {
                SuspendSite::Start
            } else {
                self.expect("at")?;
                if self.peek().text.starts_with('#') && !self.peek().quoted {
                    SuspendSite::Instruction(self.high_instruction()?)
                } else {
                    SuspendSite::Terminator(BlockId::read(self)?)
                }
            };
            self.expect("saves")?;
            let saved = Vec::read(self)?;
            let operands = if self.eat("operands") { Vec::read(self)? } else { vec![] };
            layout.continuations.push(Continuation { site, saved, operands });
            self.line()?;
        }
        Ok(layout)
    }
}

impl Writer<'_> {
    pub(super) fn frame_layout(&mut self, layout: &FrameLayout) -> Result<(), PrintError> {
        self.push("    frame.layout {\n");
        for (index, slot) in layout.slots.iter().enumerate() {
            self.push(&format!("        slot {index}: "));
            slot.ty.write(self)?;
            if slot.linear {
                self.push(" linear");
            }
            self.push("\n");
        }
        self.push("    }\n");
        for (index, continuation) in layout.continuations.iter().enumerate() {
            self.push(&format!("    continuation {index} "));
            match continuation.site {
                SuspendSite::Start => self.push("start"),
                SuspendSite::Terminator(block) => {
                    self.push("at ");
                    block.write(self)?;
                }
                SuspendSite::Instruction(instruction) => {
                    self.push("at ");
                    self.high_instruction(instruction)?;
                }
            }
            self.push(" saves ");
            continuation.saved.write(self)?;
            if !continuation.operands.is_empty() {
                self.push(" operands ");
                continuation.operands.write(self)?;
            }
            self.push("\n");
        }
        Ok(())
    }
}

/// `success(value=%v, packet=%p, deps=%d)`, `success(packet=%p, deps=%d)` or `failure(%f)`.
impl Codec for PublishOutcome {
    fn positional(_name: &str) -> bool {
        true
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, Diagnostic> {
        if r.eat("failure") {
            r.expect("(")?;
            let failure = ValueId::read(r)?;
            r.expect(")")?;
            return Ok(Self::Failure(failure));
        }
        r.expect("success")?;
        r.expect("(")?;
        let value = if r.is("value") {
            r.field("value")?;
            let value = ValueId::read(r)?;
            r.expect(",")?;
            Some(value)
        } else {
            None
        };
        r.field("packet")?;
        let packet = ValueId::read(r)?;
        r.expect(",")?;
        r.field("deps")?;
        let deps = ValueId::read(r)?;
        r.expect(")")?;
        Ok(Self::Success { value, packet, deps })
    }
    fn write(&self, w: &mut Writer<'_>) -> Result<(), PrintError> {
        match self {
            Self::Failure(failure) => {
                w.push("failure(");
                failure.write(w)?;
            }
            Self::Success { value, packet, deps } => {
                w.push("success(");
                if let Some(value) = value {
                    w.push("value=");
                    value.write(w)?;
                    w.push(", ");
                }
                w.push("packet=");
                packet.write(w)?;
                w.push(", deps=");
                deps.write(w)?;
            }
        }
        w.push(")");
        Ok(())
    }
}

/// `%cache`, `%q, %p, future %f` or `%p, %c[, task %t]` (async ABI C-4).
impl Codec for RehomeForm {
    fn positional(_name: &str) -> bool {
        true
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, Diagnostic> {
        let first = ValueId::read(r)?;
        if !r.operand_value() {
            return Ok(Self::State(first));
        }
        r.expect(",")?;
        let second = ValueId::read(r)?;
        if r.operand_keyword("future") {
            r.expect(",")?;
            r.expect("future")?;
            let future = ValueId::read(r)?;
            return Ok(Self::Scope { state: first, pending: second, future });
        }
        let task = if r.operand_keyword("task") {
            r.expect(",")?;
            r.expect("task")?;
            Some(ValueId::read(r)?)
        } else {
            None
        };
        Ok(Self::Token { pending: first, cache: second, task })
    }
    fn write(&self, w: &mut Writer<'_>) -> Result<(), PrintError> {
        match self {
            Self::State(state) => state.write(w),
            Self::Scope { state, pending, future } => {
                state.write(w)?;
                w.push(", ");
                pending.write(w)?;
                w.push(", future ");
                future.write(w)
            }
            Self::Token { pending, cache, task } => {
                pending.write(w)?;
                w.push(", ");
                cache.write(w)?;
                if let Some(task) = task {
                    w.push(", task ");
                    task.write(w)?;
                }
                Ok(())
            }
        }
    }
}

/// `holder %f, kind cache` (async ABI D25.2).
impl Codec for ServiceEntry {
    fn read(r: &mut Reader<'_>) -> Result<Self, Diagnostic> {
        r.expect("holder")?;
        let holder = ValueId::read(r)?;
        r.expect(",")?;
        r.expect("kind")?;
        let kind = ServiceKeyKind::read(r)?;
        Ok(Self { holder, kind })
    }
    fn write(&self, w: &mut Writer<'_>) -> Result<(), PrintError> {
        w.push("holder ");
        self.holder.write(w)?;
        w.push(", kind ");
        self.kind.write(w)
    }
    fn read_operand(r: &mut Reader<'_>, _name: &str, first: &mut bool) -> Result<Self, Diagnostic> {
        if !*first {
            r.expect(",")?;
        }
        *first = false;
        Self::read(r)
    }
    fn write_operand(&self, w: &mut Writer<'_>, _name: &str, first: &mut bool) -> Result<(), PrintError> {
        w.push(if *first { " " } else { ", " });
        *first = false;
        self.write(w)
    }
}

/// A schedule entry: an SSA duty token or `dynamic` (async ABI D25.3).
impl Codec for ScheduleDuty {
    fn read(r: &mut Reader<'_>) -> Result<Self, Diagnostic> {
        if r.eat("dynamic") {
            return Ok(Self::Dynamic);
        }
        Ok(Self::Value(ValueId::read(r)?))
    }
    fn write(&self, w: &mut Writer<'_>) -> Result<(), PrintError> {
        match self {
            Self::Value(value) => value.write(w),
            Self::Dynamic => {
                w.push("dynamic");
                Ok(())
            }
        }
    }
}
