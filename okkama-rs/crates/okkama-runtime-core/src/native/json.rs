//! `runtime/json`: the stream splitter natives (#136), the part of
//! `runtime/json` Sparrow routes here (`host/native_origin.rs`). Parsing,
//! nodes and writing are Sparrow's own provider (`host/native_json/`).
use super::Error;
use super::json_split::{SplitState, Splitter, Step};
use super::support::{failure, handle, int, int_value, new_handle, text};
use crate::{Value, abi::Native, heap::ArrayHeap};
use std::cell::RefCell;

pub fn execute(native: Native, args: &[Value], arrays: &ArrayHeap) -> Result<Value, Error> {
    let invalid = |message: &str| failure("core.json", "invalid_input", native, message);
    match native {
        // Items cut from a stream (#136).
        Native::JsonJsonSplitState => {
            let maximum = int(&args[0])?.to_i64().filter(|n| *n >= 0).ok_or_else(|| invalid("maximum item size"))?;
            Ok(new_handle(SplitState(RefCell::new(Splitter::new(maximum as usize)))))
        }
        Native::JsonJsonSplitFeed => {
            let elements = arrays.elements(&args[1])?;
            let bytes = elements.iter().map(Value::as_byte).collect::<Result<Vec<_>, _>>()?;
            handle::<SplitState>(&args[0])?.0.borrow_mut().feed(&bytes);
            Ok(Value::Bool(true))
        }
        Native::JsonJsonSplitFinish => {
            handle::<SplitState>(&args[0])?.0.borrow_mut().finish();
            Ok(Value::Bool(true))
        }
        Native::JsonJsonSplitNext => {
            let step = handle::<SplitState>(&args[0])?.0.borrow_mut().advance();
            Ok(text(match step {
                Step::Item => "item",
                Step::More => "more",
                Step::End => "end",
                Step::Error => "error",
            }))
        }
        Native::JsonJsonSplitItem => {
            let state = handle::<SplitState>(&args[0])?.0.borrow();
            Ok(super::support::byte_array(state.item().iter().copied()))
        }
        Native::JsonJsonSplitError => {
            let state = handle::<SplitState>(&args[0])?.0.borrow();
            Ok(text(state.error().map(|(_, message)| message.clone()).unwrap_or_default()))
        }
        Native::JsonJsonSplitOffset => {
            let state = handle::<SplitState>(&args[0])?.0.borrow();
            Ok(int_value(state.error().map_or(0, |(offset, _)| *offset as i64)))
        }
        _ => Err(Error::Unsupported(format!(
            "native provider is not implemented: {}",
            native.symbol()
        ))),
    }
}
