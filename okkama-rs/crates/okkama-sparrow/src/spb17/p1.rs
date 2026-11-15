//! Closed P1 host contracts, constructed independently of artifact function declarations.
use super::*;

#[derive(Debug)]
pub struct P1Contracts(Vec<Function>);

impl P1Contracts {
    pub fn new(objects: &[Object]) -> Result<Self, Error> {
        let mut entries = Vec::new();
        let closed = objects.iter().any(|object| object.name == "async.atomic.AtomicClosed"
            && object.error_cases == ["AtomicClosed.closed"]
            && object.fields.is_empty() && object.layout.storage_fields == [Type::Int]
            && !object.layout.may_reach_resource);
        for object in objects.iter().filter(|object| !object.error_cases.is_empty()) {
            let valid = if object.fields.is_empty() {
                object.layout.storage_fields == [Type::Int]
            } else {
                object.error_cases.len() == 1 && object.layout.storage_fields.iter()
                    .all(|ty| matches!(ty, Type::Str | Type::Int | Type::Bool))
            };
            if !valid || object.layout.may_reach_resource || object.layout.has_destroy {
                return Err(error("invalid P1 plain Error layout"));
            }
        }
        for (id, object) in objects.iter().enumerate() {
            let value = match object.name.as_str() {
                "async.atomic.Atomic_int" => Type::Int,
                "async.atomic.Atomic_string" => Type::Str,
                _ => continue,
            };
            let expected = vec![value, Type::Bool];
            if object.layout.storage_fields != expected || object.layout.may_reach_resource
                || object.layout.has_destroy || object.layout.may_async_destroy || !object.fields.is_empty()
            {
                return Err(error("invalid P1 Atomic runtime layout"));
            }
            let suffix = if value == Type::Int { "int" } else { "string" };
            let atomic = Type::Object(id as u32);
            // OIR-2 §7.2: publication closes fresh inputs with the call; reads
            // and close preserve the external lifetime of borrowed values.
            for (op, params, results, owned, failures) in [
                ("new_value", vec![value], vec![atomic], true, 0),
                ("value", vec![atomic], vec![value], false, 1),
                ("replace", vec![atomic, value], vec![], true, 1),
                ("close", vec![atomic], vec![], false, 0),
            ] {
                if failures != 0 && !closed {
                    continue;
                }
                entries.push(entry(&format!("okkama.atomic.Atomic.{op}${suffix}"), params, results, owned, failures));
            }
        }
        entries.push(entry("okkama.Error.text", vec![Type::Error], vec![Type::Str], false, 0));
        entries.push(entry("runtime/utf@1::utf8_encode/1", vec![Type::Str], vec![Type::Bytes], true, 8));
        entries.push(entry("runtime/stdio@1::stdout_write/1", vec![Type::Bytes], vec![Type::Int], true, 8));
        Ok(Self(entries))
    }

    pub fn entries(&self) -> &[Function] {
        &self.0
    }
}

impl ExternalContracts for P1Contracts {
    fn entry(&self, name: &str) -> Option<&Function> {
        self.0.iter().find(|f| f.proofs.entry.as_ref().and_then(|entry| entry.external.as_deref()) == Some(name))
    }
}

fn entry(name: &str, registers: Vec<Type>, results: Vec<Type>, owned: bool, failures: u32) -> Function {
    let mut params = (0..registers.len() as u32).map(Location::Register).collect::<Vec<_>>();
    if owned {
        params.push(Location::Service(0));
    }
    let deps = registers.iter().filter(|ty| matches!(ty, Type::Object(_) | Type::Error)).count();
    let mut services = vec![ServiceKind::Deps; deps];
    // Proof parameters precede the owning packet, as in elaborated OIR signatures.
    if owned {
        params.pop();
    }
    params.extend((0..deps as u32).map(Location::Service));
    if owned {
        params.push(Location::Service(services.len() as u32));
        services.push(ServiceKind::Packet);
    }
    Function {
        symbol: name.into(), entry_abi: if owned { EntryAbi::Owned } else { EntryAbi::Observation },
        registers, services, params, results,
        blocks: vec![], code: vec![],
        proofs: FunctionProofs {
            entry: Some(EntryContract { external: Some(name.into()), failures, ..EntryContract::default() }),
            ..FunctionProofs::default()
        },
    }
}
