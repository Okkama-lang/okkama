//! JSON trees use a flat generational arena. Native handles retain node IDs;
//! parsing, writing, identity paths, GC traversal and reclamation are iterative.
mod parser;
use crate::host::*;
use num_bigint::BigInt;
use okkama_runtime_abi::{Native, NativeFailure};
use std::{
    cell::{OnceCell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
};
#[derive(Debug)]
pub(crate) enum Node {
    Null,
    Boolean(bool),
    Number(Rc<str>),
    Text(Rc<str>),
    Array(Vec<u64>),
    Object(Names, Vec<u64>),
}
/// Below this size an object finds a member by a scan, which beats hashing.
const INDEXED_OBJECT: usize = 16;
/// Member names in document order. A large object finds a name through an
/// index built on the first lookup; otherwise walking `names()` with `get()`
/// is quadratic (#352).
#[derive(Debug)]
pub(crate) struct Names {
    list: Vec<Rc<str>>,
    // Boxed: few objects have an index, and every node pays for `Node`'s size.
    #[allow(clippy::box_collection)]
    index: OnceCell<Box<HashMap<Rc<str>, usize>>>,
}
impl Names {
    /// Rust memory of the names, for the collector's accounting.
    fn bytes(&self) -> usize {
        self.list.iter().map(|name| name.len() + 16).sum()
    }
    fn position(&self, sought: &str) -> Option<usize> {
        if self.list.len() < INDEXED_OBJECT {
            return self.list.iter().position(|name| &**name == sought);
        }
        let index = self.index.get_or_init(|| {
            let mut index = HashMap::with_capacity(self.list.len());
            for (at, name) in self.list.iter().enumerate() {
                index.entry(name.clone()).or_insert(at);
            }
            Box::new(index)
        });
        index.get(sought).copied()
    }
}
/// The first member of `node` named `name`, through the index of a big
/// object (#352), as `get` finds it (#1012); not an object: none.
fn member(node: &Node, name: &str) -> Option<u64> {
    match node {
        Node::Object(names, values) => names.position(name).map(|at| values[at]),
        _ => None,
    }
}
impl From<Vec<Rc<str>>> for Names {
    fn from(list: Vec<Rc<str>>) -> Self {
        Self { list, index: OnceCell::new() }
    }
}
impl std::ops::Deref for Names {
    type Target = [Rc<str>];
    fn deref(&self) -> &[Rc<str>] {
        &self.list
    }
}
impl Node {
    fn kind(&self) -> i64 {
        match self {
            Self::Null => 0,
            Self::Boolean(_) => 1,
            Self::Number(_) => 2,
            Self::Text(_) => 3,
            Self::Array(_) => 4,
            Self::Object(..) => 5,
        }
    }
    fn children(&self) -> &[u64] {
        match self {
            Self::Array(values) | Self::Object(_, values) => values,
            _ => &[],
        }
    }
    pub(crate) fn roots(&self, roots: &mut Vec<Slot>) {
        roots.extend(self.children().iter().map(|id| Slot {
            tag: JSON_NODE,
            bits: *id,
        }));
    }
}
#[derive(Debug)]
struct ParsedDocument {
    root: Option<u64>,
    offset: i64,
    line: i64,
    column: i64,
    message: String,
    duplicate: Option<String>,
}
#[derive(Debug)]
pub(crate) struct ParseState {
    /// The source text, UTF-8 (#468: no character vector).
    source: String,
    maximum_depth: BigInt,
    outcome: RefCell<Option<Rc<ParsedDocument>>>,
}
#[derive(Debug, Clone)]
pub(crate) enum JsonHandle {
    Node(u64),
    Parse(Rc<ParseState>),
}
impl JsonHandle {
    pub(crate) fn roots(&self, roots: &mut Vec<Slot>) {
        let root = match self {
            Self::Node(id) => Some(*id),
            Self::Parse(state) => state.outcome.borrow().as_ref().and_then(|p| p.root),
        };
        if let Some(bits) = root {
            roots.push(Slot {
                tag: JSON_NODE,
                bits,
            });
        }
    }
}
/// Character offset, line and column of byte `offset` in `source` (lines and columns from 1; CR LF is one line
/// break). The parser works in bytes (#468); what the program sees counts characters.
fn located(source: &str, offset: usize) -> (i64, i64, i64) {
    let (mut line, mut column, mut characters) = (1, 1, 0);
    let mut chars = source[..offset].chars().peekable();
    while let Some(c) = chars.next() {
        characters += 1;
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                    characters += 1;
                }
                line += 1;
                column = 1;
            }
            '\n' => {
                line += 1;
                column = 1;
            }
            _ => column += 1,
        }
    }
    (characters, line, column)
}

/// JSON as UTF-8 bytes (#135): the parser reads the decoded characters, with
/// no text in between. Bytes that are not UTF-8 fail where the valid prefix
/// ends, as a parse error there would.
fn bytes_state(bytes: &[u8], maximum_depth: BigInt) -> ParseState {
    match std::str::from_utf8(bytes) {
        Ok(text) => ParseState {
            source: text.to_string(),
            maximum_depth,
            outcome: RefCell::new(None),
        },
        Err(error) => {
            let valid = std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or_default();
            let source = valid.to_string();
            let (characters, line, column) = located(&source, source.len());
            let outcome = ParsedDocument {
                root: None,
                offset: characters,
                line,
                column,
                message: "invalid UTF-8".into(),
                duplicate: None,
            };
            ParseState {
                source,
                maximum_depth,
                outcome: RefCell::new(Some(Rc::new(outcome))),
            }
        }
    }
}

pub(crate) fn handles(native: Native) -> bool {
    // The stream splitter (#136) runs in okkama-runtime-core, through the
    // origin bridge.
    native.symbol().starts_with("runtime/json@1::") && !native.symbol().starts_with("runtime/json@1::json_split_")
}
fn failure(native: Native, code: &str, message: &str) -> ProviderError {
    ProviderError::Native(NativeFailure {
        domain: "core.json".into(),
        code: code.into(),
        operation: native.symbol().into(),
        message: message.into(),
    })
}
impl Heap {
    fn json_handle(&self, value: Slot) -> Result<JsonHandle, Error> {
        if value.kind() != HANDLE {
            return Err(Error("expected native JSON handle".into()));
        }
        match self.handles.get(value.bits)? {
            Handle::Json(state) => Ok(state.clone()),
            _ => Err(Error("expected native JSON handle".into())),
        }
    }
    fn json_id(&self, value: Slot) -> Result<u64, Error> {
        match self.json_handle(value)? {
            JsonHandle::Node(id) => {
                self.json_nodes.get(id)?;
                Ok(id)
            }
            _ => Err(Error("expected JSON node handle".into())),
        }
    }
    fn json_node(&self, value: Slot) -> Result<&Node, Error> {
        self.json_nodes.get(self.json_id(value)?)
    }
    /// Rust memory of a JSON node, charged to the collector when it is made:
    /// the arena is not the Okkama heap, so without the charge a loop that
    /// parses keeps growing it while the heap sees only tiny handle cells.
    fn json_charge(&mut self, node: &Node) {
        let bytes = std::mem::size_of::<Node>()
            + match node {
                Node::Number(text) | Node::Text(text) => text.len(),
                Node::Array(children) => children.len() * 8,
                Node::Object(names, children) => names.bytes() + children.len() * 8,
                Node::Null | Node::Boolean(_) => 0,
            };
        self.core_mut().charge_external(bytes);
    }
    fn json_handle_new(&mut self, state: JsonHandle) -> Result<Slot, Error> {
        if let JsonHandle::Parse(parse) = &state {
            // The source text lives until the state is released.
            self.core_mut().charge_external(parse.source.len());
        }
        Ok(Slot {
            tag: HANDLE,
            bits: self.handles.allocate(Handle::Json(state))?,
        })
    }
    fn json_node_value(&mut self, id: u64) -> Result<Slot, Error> {
        self.json_nodes.get(id)?;
        self.json_handle_new(JsonHandle::Node(id))
    }
    fn json_new(&mut self, node: Node) -> Result<Slot, Error> {
        self.json_charge(&node);
        let id = self.json_nodes.allocate(node)?;
        self.json_node_value(id)
    }
    fn json_shared(&self, value: Slot) -> Result<Rc<str>, Error> {
        if value.kind() != STR {
            return Err(Error("expected string slot".into()));
        }
        Ok(self.text(value)?.into())
    }
    fn json_text(&mut self, text: Rc<str>) -> Result<Slot, Error> {
        self.allocate_text(text.to_string())
    }
    fn json_member(&self, object: Slot, name: Slot) -> Result<Option<u64>, Error> {
        let name = self.text(name)?;
        Ok(member(self.json_node(object)?, name))
    }
    fn json_parsed(&mut self, value: Slot) -> Result<Rc<ParsedDocument>, Error> {
        let JsonHandle::Parse(state) = self.json_handle(value)? else {
            return Err(Error("expected JSON parse state".into()));
        };
        if let Some(outcome) = state.outcome.borrow().as_ref() {
            return Ok(outcome.clone());
        }
        let outcome = match parser::parse(&state.source, &state.maximum_depth) {
            Ok((nodes, root)) => {
                let mut ids = Vec::with_capacity(nodes.len());
                for mut node in nodes {
                    if let Node::Array(children) | Node::Object(_, children) = &mut node {
                        for child in children {
                            *child = ids[*child as usize];
                        }
                    }
                    self.json_charge(&node);
                    ids.push(self.json_nodes.allocate(node)?);
                }
                ParsedDocument {
                    root: Some(ids[root as usize]),
                    offset: 0,
                    line: 0,
                    column: 0,
                    message: String::new(),
                    duplicate: None,
                }
            }
            Err(error) => {
                let (characters, line, column) = located(&state.source, error.offset);
                ParsedDocument {
                    root: None,
                    offset: characters,
                    line,
                    column,
                    message: error.message,
                    duplicate: error.duplicate,
                }
            }
        };
        let outcome = Rc::new(outcome);
        *state.outcome.borrow_mut() = Some(outcome.clone());
        Ok(outcome)
    }
    fn json_write(&self, root: u64, indentation: Option<usize>) -> Result<String, Error> {
        write_tree(root, indentation, |id| self.json_nodes.get(id))
    }
    fn json_pointer(&self, root: u64, target: u64) -> Result<Option<String>, Error> {
        tree_pointer(root, target, |id| self.json_nodes.get(id))
    }
}
/// JSON text of the tree at `root`; compact, or indented by `indentation`
/// spaces per level.
fn write_tree<'a>(
    root: u64,
    indentation: Option<usize>,
    get: impl Fn(u64) -> Result<&'a Node, Error>,
) -> Result<String, Error> {
    // One frame per open container: pending work is bounded by depth,
    // not by the width of the containers on the way.
    struct Frame<'n> {
        node: &'n Node,
        next: usize,
    }
    let mut out = String::new();
    let mut frames: Vec<Frame> = Vec::new();
    let mut current = Some(get(root)?);
    loop {
        if let Some(node) = current.take() {
            match node {
                Node::Null => out.push_str("null"),
                Node::Boolean(value) => out.push_str(if *value { "true" } else { "false" }),
                Node::Text(text) => push_quoted(&mut out, text),
                Node::Number(text) => {
                    if let Some(parts) = crate::host::native_numbers::components(text) {
                        push_number(&mut out, &parts);
                    } else {
                        out.push_str(text);
                    }
                }
                Node::Array(_) | Node::Object(..) => {
                    out.push(if matches!(node, Node::Object(..)) { '{' } else { '[' });
                    frames.push(Frame { node, next: 0 });
                }
            }
        }
        let level = frames.len();
        let Some(frame) = frames.last_mut() else { break };
        let node = frame.node;
        let children = node.children();
        if frame.next == children.len() {
            let object = matches!(node, Node::Object(..));
            frames.pop();
            if !children.is_empty() {
                push_line(&mut out, indentation, level - 1)?;
            }
            out.push(if object { '}' } else { ']' });
            continue;
        }
        let index = frame.next;
        frame.next += 1;
        if index > 0 {
            out.push(',');
        }
        push_line(&mut out, indentation, level)?;
        if let Node::Object(names, _) = node {
            push_quoted(&mut out, &names[index]);
            out.push(':');
            if indentation.is_some() {
                out.push(' ');
            }
        }
        current = Some(get(children[index])?);
    }
    Ok(out)
}

/// The RFC 6901 pointer of `target` below `root`, if it is there.
fn tree_pointer<'a>(
    root: u64,
    target: u64,
    get: impl Fn(u64) -> Result<&'a Node, Error>,
) -> Result<Option<String>, Error> {
    let mut stack = vec![(root, 0usize)];
    let mut path: Vec<(&Node, usize)> = Vec::new();
    let mut seen = HashSet::new();
    seen.insert(root);
    if root == target {
        return Ok(Some(String::new()));
    }
    while let Some((id, index)) = stack.last_mut() {
        let node = get(*id)?;
        let children = node.children();
        if *index == children.len() {
            stack.pop();
            path.pop();
            continue;
        }
        let at = *index;
        *index += 1;
        let child = children[at];
        if !seen.insert(child) {
            continue;
        }
        path.push((node, at));
        if child == target {
            let mut pointer = String::new();
            for &(node, at) in &path {
                pointer.push('/');
                match node {
                    Node::Object(names, _) => {
                        for character in names[at].chars() {
                            match character {
                                '~' => pointer.push_str("~0"),
                                '/' => pointer.push_str("~1"),
                                other => pointer.push(other),
                            }
                        }
                    }
                    _ => {
                        use std::fmt::Write;
                        let _ = write!(pointer, "{at}");
                    }
                }
            }
            return Ok(Some(pointer));
        }
        stack.push((child, 0));
    }
    Ok(None)
}

fn push_quoted(quoted: &mut String, origin: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    quoted.reserve(origin.len() + 2);
    quoted.push('"');
    // Runs that need no escape are copied whole; every escape is ASCII, so
    // byte positions in `origin` are always character boundaries here.
    let bytes = origin.as_bytes();
    let mut run = 0;
    for (at, &byte) in bytes.iter().enumerate() {
        let escape = match byte {
            b'"' => "\\\"",
            b'\\' => "\\\\",
            b'\n' => "\\n",
            b'\r' => "\\r",
            b'\t' => "\\t",
            0..=0x1f => "",
            _ => continue,
        };
        quoted.push_str(&origin[run..at]);
        run = at + 1;
        if escape.is_empty() {
            quoted.push_str("\\u00");
            quoted.push(HEX[(byte >> 4) as usize] as char);
            quoted.push(HEX[(byte & 15) as usize] as char);
        } else {
            quoted.push_str(escape);
        }
    }
    quoted.push_str(&origin[run..]);
    quoted.push('"');
}

/// A line break and `level` indentation steps, when the output is indented.
fn push_line(out: &mut String, indentation: Option<usize>, level: usize) -> Result<(), Error> {
    if let Some(width) = indentation {
        let size = width
            .checked_mul(level)
            .ok_or_else(|| Error("JSON indentation exceeds host range".into()))?;
        out.try_reserve(
            size.checked_add(1)
                .ok_or_else(|| Error("JSON output exceeds host capacity".into()))?,
        )
        .map_err(|_| Error("JSON output exceeds host capacity".into()))?;
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', size));
    }
    Ok(())
}

/// The JSON spelling of an exact decimal (as `json.Json` writes it):
/// positional while that stays short — integers up to 21 digits, fractions
/// down to 1e-6, the thresholds of JavaScript's `Number#toString` — and the
/// canonical scientific form beyond them, so the value is never expanded.
fn push_number(out: &mut String, parts: &crate::host::native_numbers::Decimal) {
    let digits = parts.digits.as_str();
    let length = digits.len() as i128;
    let point = length + parts.exponent as i128;
    let positional = if parts.exponent >= 0 { point <= 21 } else { point > -6 };
    if !positional {
        out.push_str(&crate::host::native_numbers::canonical(parts));
        return;
    }
    if parts.sign < 0 {
        out.push('-');
    }
    if parts.exponent >= 0 {
        out.push_str(digits);
        out.extend(std::iter::repeat_n('0', parts.exponent as usize));
    } else if point > 0 {
        let (whole, fraction) = digits.split_at(point as usize);
        out.push_str(whole);
        out.push('.');
        out.push_str(fraction);
    } else {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', (-point) as usize));
        out.push_str(digits);
    }
}

impl Heap {
    pub(crate) fn native_json(&mut self, native: Native, args: &[Slot]) -> Result<Outcome, Error> {
        match self.json_invoke(native, args) {
            Ok(value) => Ok(Outcome::Value(value)),
            Err(ProviderError::Engine(error)) => Err(error),
            Err(ProviderError::Native(failure)) => {
                let payload = self.native_failure_new(failure)?;
                self.failure_new(crate::host::FailureKind::Native(payload))
                    .map(Outcome::Failure)
            }
        }
    }
    fn json_invoke(&mut self, native: Native, args: &[Slot]) -> Result<Slot, ProviderError> {
        use Native::*;
        let invalid = |message| failure(native, "invalid_input", message);
        let index =
            |heap: &Heap, value: Slot, len: usize, message| -> Result<usize, ProviderError> {
                heap.integer(value)?
                    .to_i64()
                    .and_then(|n| usize::try_from(n).ok())
                    .filter(|n| *n < len)
                    .ok_or_else(|| invalid(message))
            };
        if matches!(
            native,
            JsonJsonParseValid
                | JsonJsonParseHasDuplicate
                | JsonJsonParseOffset
                | JsonJsonParseLine
                | JsonJsonParseColumn
                | JsonJsonParseMessage
                | JsonJsonParseDuplicate
                | JsonJsonParseRoot
        ) {
            let parsed = self.json_parsed(args[0])?;
            return Ok(match native {
                JsonJsonParseValid => Slot::boolean(parsed.root.is_some()),
                JsonJsonParseHasDuplicate => Slot::boolean(parsed.duplicate.is_some()),
                JsonJsonParseOffset => Slot::small(parsed.offset),
                JsonJsonParseLine => Slot::small(parsed.line),
                JsonJsonParseColumn => Slot::small(parsed.column),
                JsonJsonParseMessage => self.allocate_text(parsed.message.clone())?,
                JsonJsonParseDuplicate => {
                    self.allocate_text(parsed.duplicate.clone().unwrap_or_default())?
                }
                JsonJsonParseRoot => self.json_node_value(parsed.root.ok_or_else(|| {
                    failure(native, "invalid_data", "invalid JSON has no root")
                })?)?,
                _ => unreachable!(),
            });
        }
        let value = match native {
            JsonJsonParseState => {
                let maximum_depth = match self.integer(args[1])? {
                    Integer::Small(n) => BigInt::from(n),
                    Integer::Big(n) => (*n).clone(),
                };
                let source = self.text(args[0])?.to_string();
                self.json_handle_new(JsonHandle::Parse(Rc::new(ParseState {
                    source,
                    maximum_depth,
                    outcome: RefCell::new(None),
                })))?
            }
            JsonJsonParseBytesState => {
                let maximum_depth = match self.integer(args[1])? {
                    Integer::Small(n) => BigInt::from(n),
                    Integer::Big(n) => (*n).clone(),
                };
                let state = bytes_state(&self.byte_slice(args[0])?, maximum_depth);
                self.json_handle_new(JsonHandle::Parse(Rc::new(state)))?
            }
            JsonJsonNodeKind => Slot::small(self.json_node(args[0])?.kind()),
            JsonJsonNodeBoolean => match self.json_node(args[0])? {
                Node::Boolean(value) => Slot::boolean(*value),
                _ => return Err(invalid("JSON node is not boolean")),
            },
            JsonJsonNodeText | JsonJsonNodeNumber => {
                let text = match (native, self.json_node(args[0])?) {
                    (JsonJsonNodeText, Node::Text(text))
                    | (JsonJsonNodeNumber, Node::Number(text)) => text.clone(),
                    _ => return Err(invalid("JSON node has the wrong kind")),
                };
                self.json_text(text)?
            }
            JsonJsonNodeSize => match self.json_node(args[0])? {
                Node::Array(values) | Node::Object(_, values) => Slot::small(values.len() as i64),
                _ => return Err(invalid("JSON node has no children")),
            },
            JsonJsonNodeItem => {
                let Node::Array(values) = self.json_node(args[0])? else {
                    return Err(invalid("JSON node is not an array"));
                };
                let at = index(
                    self,
                    args[1],
                    values.len(),
                    "JSON array index is outside the node",
                )?;
                let child = values[at];
                self.json_node_value(child)?
            }
            JsonJsonMemberValue => {
                let Node::Object(_, values) = self.json_node(args[0])? else {
                    return Err(invalid("JSON node is not an object"));
                };
                let at = index(
                    self,
                    args[1],
                    values.len(),
                    "JSON member index is outside the node",
                )?;
                let child = values[at];
                self.json_node_value(child)?
            }
            JsonJsonMemberPosition => {
                let Node::Object(names, _) = self.json_node(args[0])? else {
                    return Err(invalid("JSON node is not an object"));
                };
                let sought = self.text(args[1])?;
                Slot::small(names.position(sought).map_or(-1, |i| i as i64))
            }
            JsonJsonMemberText
            | JsonJsonMemberSpelling
            | JsonJsonMemberBoolean
            | JsonJsonMemberInt => {
                let member = self
                    .json_member(args[0], args[1])?
                    .map(|id| self.json_nodes.get(id))
                    .transpose()?;
                match (native, member) {
                    (JsonJsonMemberText, Some(Node::Text(text)))
                    | (JsonJsonMemberSpelling, Some(Node::Number(text))) => {
                        let text = text.clone();
                        self.json_text(text)?
                    }
                    (JsonJsonMemberBoolean, Some(Node::Boolean(value))) => Slot::boolean(*value),
                    (JsonJsonMemberInt, Some(Node::Number(text)))
                        if text.trim_start_matches('-').len() <= 18
                            && !text.trim_start_matches('-').is_empty()
                            && text
                                .trim_start_matches('-')
                                .bytes()
                                .all(|b| b.is_ascii_digit()) =>
                    {
                        Slot::small(text.parse::<i64>().map_err(|_| invalid("JSON integer"))?)
                    }
                    _ => {
                        return Err(invalid(match native {
                            JsonJsonMemberText => "JSON member is missing or not a string",
                            JsonJsonMemberSpelling => "JSON member is missing or not a number",
                            JsonJsonMemberBoolean => "JSON member is missing or not boolean",
                            _ => "JSON member is missing or not a short integer",
                        }));
                    }
                }
            }
            JsonJsonTextNode => {
                let text = self.json_shared(args[0])?;
                self.json_new(Node::Text(text))?
            }
            JsonJsonNumberNode => {
                crate::host::native_numbers::components(self.text(args[0])?)
                    .ok_or_else(|| invalid("not a decimal number"))?;
                let text = self.json_shared(args[0])?;
                self.json_new(Node::Number(text))?
            }
            JsonJsonBooleanNode => self.json_new(Node::Boolean(args[0].bits != 0))?,
            JsonJsonNullNode => self.json_new(Node::Null)?,
            JsonJsonArrayNode => {
                let children = self
                    .elements(args[0])?
                    .iter()
                    .map(|s| self.json_id(*s))
                    .collect::<Result<Vec<_>, _>>()?;
                self.json_new(Node::Array(children))?
            }
            JsonJsonObjectNode => {
                let names = self
                    .elements(args[0])?
                    .iter()
                    .map(|s| self.json_shared(*s))
                    .collect::<Result<Vec<_>, _>>()?;
                let children = self
                    .elements(args[1])?
                    .iter()
                    .map(|s| self.json_id(*s))
                    .collect::<Result<Vec<_>, _>>()?;
                if names.len() != children.len() {
                    return Err(invalid("JSON member names and values differ in number"));
                }
                let mut seen = HashSet::with_capacity(names.len());
                if !names.iter().all(|n| seen.insert(n.as_ref())) {
                    return Err(invalid("duplicate JSON member name"));
                }
                self.json_new(Node::Object(names.into(), children))?
            }
            JsonJsonNodeWrite => {
                let indentation = self
                    .integer(args[1])?
                    .to_i64()
                    .ok_or_else(|| invalid("JSON indentation is out of range"))?;
                let text =
                    self.json_write(self.json_id(args[0])?, usize::try_from(indentation).ok())?;
                self.allocate_text(text)?
            }
            JsonJsonNodePointer => {
                let path = self
                    .json_pointer(self.json_id(args[0])?, self.json_id(args[1])?)?
                    .ok_or_else(|| invalid("JSON node is not inside the document"))?;
                self.allocate_text(path)?
            }
            JsonJsonMemberValues | JsonJsonNodeItems => {
                let children = match (native, self.json_node(args[0])?) {
                    (JsonJsonMemberValues, Node::Object(_, values))
                    | (JsonJsonNodeItems, Node::Array(values)) => values.clone(),
                    _ => {
                        return Err(invalid(if native == JsonJsonNodeItems {
                            "JSON node is not an array"
                        } else {
                            "JSON node is not an object"
                        }));
                    }
                };
                let slots = children
                    .into_iter()
                    .map(|id| self.json_node_value(id))
                    .collect::<Result<Vec<_>, _>>()?;
                self.allocate_array(slots)?
            }
            JsonJsonMemberNames => {
                let Node::Object(names, _) = self.json_node(args[0])? else {
                    return Err(invalid("JSON node is not an object"));
                };
                let names = names.to_vec();
                let slots = names
                    .into_iter()
                    .map(|s| self.json_text(s))
                    .collect::<Result<Vec<_>, _>>()?;
                self.allocate_array(slots)?
            }
            JsonJsonRepeatedName => {
                let names = self.elements(args[0])?;
                let mut seen = HashSet::with_capacity(names.len());
                let mut repeated = names.len();
                for (i, &name) in names.iter().enumerate() {
                    if !seen.insert(self.text(name)?) {
                        repeated = i;
                        break;
                    }
                }
                Slot::small(repeated as i64)
            }
            _ => return Err(Error("invalid JSON provider".into()).into()),
        };
        Ok(value)
    }
}

#[cfg(test)]
mod source_tests {
    /// #468: the parser counts bytes; diagnostics count characters, lines and columns.
    #[test]
    fn byte_offsets_become_character_positions() {
        let text = "\"é\"\r\n x";
        let error = super::parser::parse(text, &num_bigint::BigInt::from(10)).unwrap_err();
        assert_eq!(error.offset, 7, "byte offset of x");
        assert_eq!(super::located(text, error.offset), (6, 2, 2));
        assert_eq!(super::located("ab", 2), (2, 1, 3));
    }

    #[test]
    fn strings_and_numbers_read_the_utf8_source() {
        let (nodes, root) = super::parser::parse("{\"ключ\": [\"дом 🏠\", -1.5e3]}", &num_bigint::BigInt::from(10)).unwrap();
        assert_eq!(root as usize, nodes.len() - 1);
        let texts: Vec<String> = nodes.iter().filter_map(|node| match node {
            super::Node::Text(text) => Some(text.to_string()),
            super::Node::Number(number) => Some(number.to_string()),
            _ => None,
        }).collect();
        assert_eq!(texts, ["дом 🏠", "-1.5e3"]);
    }

    /// #1012: the member fast paths (`text(name)` and the like) find the
    /// first member of that name through a big object's index, not a scan.
    #[test]
    fn member_lookup_uses_the_index_of_a_big_object() {
        use std::rc::Rc;
        let count = super::INDEXED_OBJECT * 4;
        let mut names: Vec<Rc<str>> = (0..count).map(|i| Rc::from(format!("k{i}"))).collect();
        names.push(Rc::from("k3"));
        let values: Vec<u64> = (0..=count as u64).collect();
        let object = super::Node::Object(names.into(), values);
        assert_eq!(super::member(&object, "k7"), Some(7));
        assert_eq!(super::member(&object, "k3"), Some(3), "the first of two");
        assert_eq!(super::member(&object, "none"), None);
        let super::Node::Object(names, _) = &object else { unreachable!() };
        assert!(names.index.get().is_some(), "looked up through the index");
        let small = super::Node::Object(vec![Rc::from("a")].into(), vec![9]);
        assert_eq!(super::member(&small, "a"), Some(9));
        assert_eq!(super::member(&super::Node::Array(vec![1]), "a"), None);
    }

    /// #1012: the parser's own depth limit fails like a requested one, at
    /// the value that is too deep, so the document reports a real line and
    /// column; a limit of zero or below refuses the first value.
    #[test]
    fn the_hard_depth_limit_fails_like_a_requested_one() {
        use num_bigint::BigInt;
        let deep = "[".repeat(100_001);
        let hard = super::parser::parse(&deep, &BigInt::from(1_000_000)).unwrap_err();
        assert_eq!(hard.message, "maximum JSON depth exceeded");
        assert_eq!(hard.offset, 100_000);
        assert_eq!(super::located(&deep, hard.offset), (100_000, 1, 100_001));
        let asked = super::parser::parse("[[[1]]]", &BigInt::from(2)).unwrap_err();
        assert_eq!((asked.message.as_str(), asked.offset), ("maximum JSON depth exceeded", 2));
        let huge = BigInt::from(u64::MAX) * BigInt::from(u64::MAX);
        assert!(super::parser::parse("[[1]]", &huge).is_ok());
        assert!(super::parser::parse("1", &BigInt::from(0)).is_err());
        assert!(super::parser::parse("1", &BigInt::from(-5)).is_err());
        assert!(super::parser::parse("1", &BigInt::from(1)).is_ok());
    }
}


#[cfg(test)]
mod writer_tests {
    use super::{parser, push_number, push_quoted, tree_pointer, write_tree, Node};
    use crate::host::native_numbers::{canonical, components};

    fn tree(text: &str) -> (Vec<Node>, u64) {
        parser::parse(text, &num_bigint::BigInt::from(64)).unwrap()
    }
    fn write(text: &str, indentation: Option<usize>) -> String {
        let (nodes, root) = tree(text);
        write_tree(root, indentation, |id| Ok(&nodes[id as usize])).unwrap()
    }

    /// #466: compact and indented text, empty and nested containers, member order.
    #[test]
    fn writes_compact_and_indented_text() {
        let source = r#"{"a": [1, [], {}, {"b": null}], "c": true, "d": "x"}"#;
        assert_eq!(write(source, None), r#"{"a":[1,[],{},{"b":null}],"c":true,"d":"x"}"#);
        assert_eq!(
            write(source, Some(2)),
            "{\n  \"a\": [\n    1,\n    [],\n    {},\n    {\n      \"b\": null\n    }\n  ],\n  \"c\": true,\n  \"d\": \"x\"\n}"
        );
        assert_eq!(write("[[[]]]", Some(1)), "[\n [\n  []\n ]\n]");
        assert_eq!(write("[1,2]", Some(0)), "[\n1,\n2\n]");
        assert_eq!(write("\"top\"", Some(4)), "\"top\"");
    }

    /// Escapes are written in place: named ones, `\u00xx` for other controls,
    /// everything else (multi-byte characters, DEL) as is, around unescaped runs.
    #[test]
    fn quoting_escapes_in_place() {
        let mut out = String::new();
        push_quoted(&mut out, "a\"b\\c\nd\re\tf\u{0}\u{1f}\u{7f}é🏠/");
        assert_eq!(out, "\"a\\\"b\\\\c\\nd\\re\\tf\\u0000\\u001f\u{7f}é🏠/\"");
        out.clear();
        push_quoted(&mut out, "");
        assert_eq!(out, "\"\"");
        out.clear();
        push_quoted(&mut out, "\u{b}");
        assert_eq!(out, "\"\\u000b\"");
    }

    /// RFC 6901: `~` becomes `~0` and `/` becomes `~1`; array steps are indices.
    #[test]
    fn pointer_escapes_tokens() {
        let (nodes, root) = tree(r#"{"a/b": {"m~n": [0, {"": "x"}]}}"#);
        let get = |id: u64| Ok(&nodes[id as usize]);
        let target = nodes.iter().position(|node| matches!(node, Node::Text(text) if &**text == "x")).unwrap() as u64;
        assert_eq!(tree_pointer(root, target, get).unwrap().as_deref(), Some("/a~1b/m~0n/1/"));
        assert_eq!(tree_pointer(root, root, get).unwrap().as_deref(), Some(""));
        let (other, other_root) = tree("[7]");
        let _ = other;
        assert_eq!(tree_pointer(root, other_root + 100, get).unwrap(), None);
    }

    /// The number spelling: positional up to 21 integer digits and down to
    /// 1e-6, canonical scientific beyond — checked at every threshold.
    #[test]
    fn numbers_switch_to_scientific_at_the_thresholds() {
        let spell = |text: &str| {
            let mut out = String::new();
            push_number(&mut out, &components(text).unwrap());
            out
        };
        assert_eq!(spell("1e20"), "100000000000000000000");
        assert_eq!(spell("-1e20"), "-100000000000000000000");
        assert_eq!(spell("1e21"), canonical(&components("1e21").unwrap()));
        assert_eq!(spell("12.5"), "12.5");
        assert_eq!(spell("-0.5"), "-0.5");
        assert_eq!(spell("1e-6"), "0.000001");
        assert_eq!(spell("-15e-7"), "-0.0000015");
        assert_eq!(spell("1e-7"), canonical(&components("1e-7").unwrap()));
        assert_eq!(spell("0"), "0");
    }
}
