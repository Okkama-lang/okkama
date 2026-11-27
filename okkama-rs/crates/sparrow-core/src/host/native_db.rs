//! Shared immutable database cells. Driver results own Rust data directly;
//! captures and schemas never retain VM objects or cursor lifetimes.
use crate::host::*;
use num_bigint::BigInt;
use num_traits::ToPrimitive;
use okkama_runtime_abi::{Native, NativeFailure};
use std::{collections::HashMap, rc::Rc};
#[derive(Debug, Clone)]
pub(crate) enum Cell {
    Null,
    Boolean(bool),
    Integer(Rc<BigInt>),
    Floating(f64),
    Decimal(Rc<str>),
    Text(Rc<str>),
    Bytes(Rc<[u8]>),
}
impl Cell {
    pub(crate) fn kind(&self) -> i64 {
        match self {
            Self::Null => 0,
            Self::Boolean(_) => 1,
            Self::Integer(_) => 2,
            Self::Floating(_) => 3,
            Self::Decimal(_) => 4,
            Self::Text(_) => 5,
            Self::Bytes(_) => 6,
        }
    }
    fn size(&self) -> usize {
        match self {
            Self::Null => 0,
            Self::Boolean(_) => 1,
            Self::Integer(n) => n.to_string().len(),
            Self::Floating(n) => crate::scalar::float_text(*n).len(),
            Self::Decimal(s) | Self::Text(s) => s.len(),
            Self::Bytes(b) => b.len(),
        }
    }
}
#[derive(Debug, Default)]
pub(crate) struct Capture {
    kind: i64,
    started: bool,
    faulted: bool,
    poisoned: bool,
    sealed: bool,
    value: Option<Cell>,
}
#[derive(Debug)]
pub(crate) enum DbHandle {
    Cell(Cell),
    Capture(Capture),
    /// Position of each column name, `-2` for a name several columns share.
    /// The names come from the database server, so the map takes std's keyed
    /// hash, never `FastHasher` (#1103).
    Schema(HashMap<Rc<str>, i64>),
}
pub(crate) fn handles(native: Native) -> bool {
    native.symbol().starts_with("runtime/db@1::")
}
fn invalid(native: Native) -> ProviderError {
    ProviderError::Native(NativeFailure {
        domain: "core.db".into(),
        code: "invalid_input".into(),
        operation: native.symbol().into(),
        message: "invalid database cell or projection".into(),
    })
}
impl Heap {
    fn db_handle(&self, value: Slot) -> Result<&DbHandle, Error> {
        if value.kind() != HANDLE {
            return Err(Error("expected database handle".into()));
        }
        match self.handles.get(value.bits)? {
            Handle::Db(handle) => Ok(handle),
            _ => Err(Error("expected database handle".into())),
        }
    }
    pub(crate) fn db_cell(&self, value: Slot) -> Result<&Cell, Error> {
        match self.db_handle(value)? {
            DbHandle::Cell(cell) => Ok(cell),
            _ => Err(Error("expected database cell".into())),
        }
    }
    fn db_new(&mut self, state: DbHandle) -> Result<Slot, Error> {
        Ok(Slot {
            tag: HANDLE,
            bits: self.handles.allocate(Handle::Db(state))?,
        })
    }
    pub(crate) fn db_cell_new(&mut self, cell: Cell) -> Result<Slot, Error> {
        self.db_new(DbHandle::Cell(cell))
    }
    fn db_string(&self, value: Slot) -> Result<Rc<str>, Error> {
        Ok(self.text(value)?.into())
    }
    pub(crate) fn native_db(&mut self, n: Native, a: &[Slot]) -> Result<Outcome, Error> {
        match self.db_execute(n, a) {
            Ok(value) => Ok(Outcome::Value(value)),
            Err(ProviderError::Engine(error)) => Err(error),
            Err(ProviderError::Native(error)) => {
                let payload = self.native_failure_new(error)?;
                self.failure_new(crate::host::FailureKind::Native(payload))
                    .map(Outcome::Failure)
            }
        }
    }
    fn db_execute(&mut self, n: Native, a: &[Slot]) -> Result<Slot, ProviderError> {
        use Native::*;
        // The `db_capture*` natives have no std caller since 42f88cd21
        // (`std/db/value.ok` keeps its own capture on `Atomic` flags); they
        // stay as declared in `runtime/db`, which loses nothing (#1012).
        if n == DbCapture {
            let kind = self
                .int64(a[0])?
                .filter(|n| (0..=6).contains(n))
                .ok_or_else(|| invalid(n))?;
            return Ok(self.db_new(DbHandle::Capture(Capture {
                kind,
                ..Capture::default()
            }))?);
        }
        if matches!(
            n,
            DbCaptureEmit | DbCapturePublish | DbCaptureComplete | DbCaptureValue
        ) {
            // Read and validate arguments before borrowing mutable capture state.
            let kind = if n == DbCaptureEmit {
                self.int64(a[1])?
            } else {
                None
            };
            let cell = if n == DbCapturePublish {
                Some(self.db_cell(a[1])?.clone())
            } else {
                None
            };
            if !matches!(self.db_handle(a[0])?, DbHandle::Capture(_)) {
                return Err(invalid(n));
            }
            let Handle::Db(DbHandle::Capture(c)) = self.handles.get_mut(a[0].bits)? else {
                unreachable!()
            };
            return match n {
                DbCaptureEmit => Ok(Slot::small(if c.sealed {
                    1
                } else if c.faulted || c.started || kind != Some(c.kind) {
                    c.faulted = true;
                    c.poisoned = true;
                    2
                } else {
                    c.started = true;
                    c.faulted = true;
                    0
                })),
                DbCapturePublish => {
                    let cell = cell.expect("validated cell");
                    let valid = !c.sealed
                        && !c.poisoned
                        && c.started
                        && c.value.is_none()
                        && c.kind == cell.kind();
                    if valid {
                        c.value = Some(cell);
                        c.faulted = false;
                    } else if !c.sealed {
                        c.faulted = true;
                    }
                    Ok(Slot::boolean(valid))
                }
                DbCaptureComplete => {
                    let valid = c.started && !c.faulted && c.value.is_some();
                    if valid && a[1].bits != 0 {
                        c.sealed = true;
                    }
                    Ok(Slot::boolean(valid))
                }
                DbCaptureValue => {
                    if c.faulted {
                        return Err(invalid(n));
                    }
                    let cell = c.value.clone().ok_or_else(|| invalid(n))?;
                    Ok(self.db_cell_new(cell)?)
                }
                _ => unreachable!(),
            };
        }
        let cell = match n {
            DbNull => Some(Cell::Null),
            DbBoolean => Some(Cell::Boolean(a[0].bits != 0)),
            DbInteger => {
                let s = self.text(a[0])?;
                let digits = s.strip_prefix('-').unwrap_or(s);
                if s.len() > 4096
                    || digits.is_empty()
                    || !digits.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(invalid(n));
                }
                Some(Cell::Integer(Rc::new(
                    BigInt::parse_bytes(s.as_bytes(), 10).ok_or_else(|| invalid(n))?,
                )))
            }
            DbFloating => {
                let f = f64::from_bits(a[0].bits);
                if !f.is_finite() {
                    return Err(invalid(n));
                }
                Some(Cell::Floating(f))
            }
            DbDecimal | DbText => {
                let s = self.db_string(a[0])?;
                if n == DbDecimal {
                    if s.len() > 4096 || crate::host::native_numbers::components(&s).is_none() {
                        return Err(invalid(n));
                    }
                    Some(Cell::Decimal(s))
                } else {
                    if s.len() > 8388608 {
                        return Err(invalid(n));
                    }
                    Some(Cell::Text(s))
                }
            }
            DbBytes => {
                if self.array_length(a[0])? > 8388608 {
                    return Err(invalid(n));
                }
                Some(Cell::Bytes(self.native_bytes(a[0])?.into()))
            }
            _ => None,
        };
        if let Some(cell) = cell {
            return Ok(self.db_cell_new(cell)?);
        }
        if n == DbSchema {
            let names = self.elements(a[0])?;
            if names.len() > 1024 {
                return Err(invalid(n));
            }
            let mut positions = HashMap::with_capacity(names.len());
            for (i, name) in names.iter().enumerate() {
                let name = self.db_string(*name)?;
                if name.len() > 4096 {
                    return Err(invalid(n));
                }
                positions
                    .entry(name)
                    .and_modify(|p| *p = -2)
                    .or_insert(i as i64);
            }
            return Ok(self.db_new(DbHandle::Schema(positions))?);
        }
        if n == DbPosition {
            let DbHandle::Schema(schema) = self.db_handle(a[0])? else {
                return Err(invalid(n));
            };
            return Ok(Slot::small(*schema.get(self.text(a[1])?).unwrap_or(&-1)));
        }
        if n == DbCellsBytes {
            let mut size = 0;
            for value in self.elements(a[0])?.iter() {
                size += self.db_cell(*value)?.size();
                if size > 8388608 {
                    break;
                }
            }
            return Ok(Slot::small(size as i64));
        }
        let cell = self.db_cell(a[0])?;
        let value = match (n, cell) {
            (DbKind, c) => Slot::small(c.kind()),
            (DbBooleanValue, Cell::Boolean(v)) => Slot::boolean(*v),
            (DbIntegerValue, Cell::Integer(v)) => {
                let s = v.to_string();
                self.allocate_text(s)?
            }
            (DbIntValid, Cell::Integer(v)) => Slot::boolean(v.to_i64().is_some()),
            (DbIntValid, _) => Slot::boolean(false),
            (DbIntValue, Cell::Integer(v)) => Slot::small(v.to_i64().ok_or_else(|| invalid(n))?),
            (DbFloatingValue, Cell::Floating(v)) => Slot {
                tag: FLOAT,
                bits: v.to_bits(),
            },
            (DbTextValue, Cell::Text(v)) | (DbDecimalValue, Cell::Decimal(v)) => {
                let text = v.to_string();
                self.allocate_text(text)?
            }
            (DbBytesValue, Cell::Bytes(v)) => {
                let bytes = v.to_vec();
                self.native_byte_array(bytes)?
            }
            _ => return Err(invalid(n)),
        };
        Ok(value)
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::word::FastHasher;
    use std::hash::{BuildHasher, Hasher};

    /// `count` distinct printable names of 16 bytes that a database server
    /// could send, all with one hash under the unkeyed `FastHasher`: its state
    /// after the first word is known, so a second word cancelling it sends
    /// every name to the same state (#1103).
    fn colliding_names(count: usize) -> Vec<String> {
        const K: u64 = 0x517c_c1b7_2722_0a95;
        let printable = |w: u64| w.to_le_bytes().iter().all(|b| (0x20..0x7f).contains(b));
        let mut names = Vec::new();
        let mut x: u64 = 0;
        while names.len() < count {
            x += 1;
            let first = u64::from_le_bytes(x.wrapping_mul(0x9e37_79b9_7f4a_7c15).to_le_bytes().map(|b| 0x20 + b % 95));
            let second = (first.wrapping_mul(K)).rotate_left(5) ^ 0x4141_4141_4141_4141;
            if printable(second) {
                let bytes = [first.to_le_bytes(), second.to_le_bytes()].concat();
                names.push(String::from_utf8(bytes).unwrap());
            }
        }
        names
    }

    /// The unkeyed hash of a name as `str` hashing feeds it: its 8-byte
    /// little-endian words, then the 0xff terminator.
    fn unkeyed(name: &str) -> u64 {
        let mut h = FastHasher::default();
        for chunk in name.as_bytes().chunks(8) {
            let mut word = [0; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            h.write_u64(u64::from_le_bytes(word));
        }
        h.write_u8(0xff);
        h.finish()
    }

    fn with_db<T>(body: impl FnOnce(&mut Heap) -> T) -> T {
        let program = okkama_sparrow::Program::default();
        let mut runtime = Runtime::new(crate::program::Loaded::new(program)).unwrap();
        with_host(&mut runtime, false, body)
    }

    fn schema(heap: &mut Heap, names: &[String]) -> Result<Slot, ProviderError> {
        let names = names.iter().map(|n| heap.allocate_text(n.clone()).unwrap()).collect();
        let names = heap.allocate_array(names).unwrap();
        heap.db_execute(Native::DbSchema, &[names])
    }

    fn refused(result: Result<Slot, ProviderError>) -> bool {
        matches!(result, Err(ProviderError::Native(failure)) if failure.code == "invalid_input")
    }

    fn position(heap: &mut Heap, schema: Slot, name: &str) -> i64 {
        let name = heap.allocate_text(name.into()).unwrap();
        let Outcome::Value(at) = heap.native_db(Native::DbPosition, &[schema, name]).unwrap() else {
            panic!("db_position failed");
        };
        heap.int64(at).unwrap().unwrap()
    }

    /// Column names chosen to collide under a predictable hash resolve as any
    /// others do (each to its position, a shared name to -2, an absent one to
    /// -1), and the schema's hash tells them apart, so a server sending them
    /// gets no quadratic table.
    #[test]
    fn column_names_colliding_under_an_unkeyed_hash_get_distinct_hashes_1103() {
        let mut names = colliding_names(64);
        let first = unkeyed(&names[0]);
        assert!(names.iter().all(|n| unkeyed(n) == first), "the names must collide under FastHasher");
        names.push(names[3].clone());
        with_db(|heap| {
            let Ok(state) = schema(heap, &names) else { panic!("db_schema failed") };
            for (i, name) in names[..64].iter().enumerate() {
                assert_eq!(position(heap, state, name), if i == 3 { -2 } else { i as i64 }, "{name:?}");
            }
            assert_eq!(position(heap, state, "absent"), -1);
            assert_eq!(position(heap, state, &names[0][..15]), -1);
            let Ok(DbHandle::Schema(map)) = heap.db_handle(state) else { panic!("not a schema") };
            let hashes: std::collections::HashSet<u64> =
                names[..64].iter().map(|n| map.hasher().hash_one(n.as_str())).collect();
            assert_eq!(hashes.len(), 64, "the schema's hash must separate names an attacker can collide");
        });
    }

    /// A schema keeps its input limits: at most 1024 columns, names of at
    /// most 4096 bytes; anything larger is refused as invalid input.
    #[test]
    fn a_schema_refuses_more_than_1024_columns_or_names_over_4096_bytes_1103() {
        with_db(|heap| {
            let wide: Vec<String> = (0..1024).map(|i| format!("c{i}")).collect();
            assert!(schema(heap, &wide).is_ok());
            let wider: Vec<String> = (0..1025).map(|i| format!("c{i}")).collect();
            assert!(refused(schema(heap, &wider)));
            assert!(schema(heap, &["n".repeat(4096)]).is_ok());
            assert!(refused(schema(heap, &["n".repeat(4097)])));
        });
    }
}
