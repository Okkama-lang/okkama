//! The program's environment is the run's host session
//! (`okkama_runtime_core::host`, #40), shared with the bridged origin natives;
//! the process environment is never changed.
use crate::host::{
    Error,
    {Outcome, ProviderError},
    *,
};
use okkama_runtime_abi::{Native, NativeFailure};
use okkama_runtime_core::host;
use std::collections::BTreeMap;

pub(crate) fn handles(n: Native) -> bool {
    n.symbol().starts_with("runtime/environment@1::")
}
fn failed(n: Native, code: &str, message: &str) -> ProviderError {
    ProviderError::Native(NativeFailure {
        domain: "core.environment".into(),
        code: code.into(),
        operation: n.symbol().into(),
        message: message.into(),
    })
}
fn name(n: Native, s: &str) -> Result<String, ProviderError> {
    if s.is_empty() {
        return Err(failed(
            n,
            "invalid_input",
            "Environment name must not be empty",
        ));
    }
    if s.contains('\0') {
        return Err(failed(
            n,
            "invalid_input",
            "Environment name contains a zero byte",
        ));
    }
    if s.contains('=') {
        return Err(failed(
            n,
            "invalid_input",
            "Environment name contains \"=\"",
        ));
    }
    Ok(if cfg!(windows) {
        s.to_uppercase()
    } else {
        s.into()
    })
}
fn value(n: Native, s: &str) -> Result<String, ProviderError> {
    if s.contains('\0') {
        return Err(failed(
            n,
            "invalid_input",
            "Environment value contains a zero byte",
        ));
    }
    Ok(s.into())
}
impl Heap {
    fn environment_map(
        &self,
        n: Native,
        slot: Slot,
    ) -> Result<BTreeMap<String, String>, ProviderError> {
        if slot.kind() != MAP {
            return Err(Error("expected environment map".into()).into());
        }
        let mut out = BTreeMap::new();
        for (key, content) in self.map_entries(slot)? {
            let name = name(n, self.text(key)?)?;
            let content = value(n, self.text(content)?)?;
            if out.insert(name, content).is_some() {
                return Err(failed(
                    n,
                    "invalid_input",
                    "Environment contains names that collide on this platform",
                ));
            }
        }
        Ok(out)
    }
    fn environment_publish(&mut self, values: BTreeMap<String, String>) -> Result<Slot, Error> {
        let mut entries = Vec::with_capacity(values.len());
        for (key, value) in values {
            entries.push((self.allocate_text(key)?, self.allocate_text(value)?));
        }
        self.map_new(entries.into_iter())
    }
    pub(crate) fn native_environment(&mut self, n: Native, a: &[Slot]) -> Result<Outcome, Error> {
        match self.environment_execute(n, a) {
            Ok(v) => Ok(Outcome::Value(v)),
            Err(ProviderError::Engine(e)) => Err(e),
            Err(ProviderError::Native(e)) => {
                let payload = self.native_failure_new(e)?;
                self.failure_new(crate::host::FailureKind::Native(payload))
                    .map(Outcome::Failure)
            }
        }
    }
    fn environment_execute(&mut self, n: Native, a: &[Slot]) -> Result<Slot, ProviderError> {
        use Native::*;
        match n {
            EnvironmentEnvironmentSnapshot => {
                let mut out = BTreeMap::new();
                for (key, content) in host::environment().iter() {
                    let key = name(n, key)?;
                    let content = value(n, content)?;
                    if out.insert(key, content).is_some() {
                        return Err(failed(
                            n,
                            "invalid_data",
                            "Process environment contains colliding variable names",
                        ));
                    }
                }
                Ok(self.environment_publish(out)?)
            }
            EnvironmentEnvironmentNormalize
            | EnvironmentEnvironmentAdd
            | EnvironmentEnvironmentDiff => {
                let mut out = self.environment_map(n, a[0])?;
                if n == EnvironmentEnvironmentAdd {
                    out.insert(
                        name(n, self.text(a[1])?)?,
                        value(n, self.text(a[2])?)?,
                    );
                }
                if n == EnvironmentEnvironmentDiff {
                    out.remove(&name(n, self.text(a[1])?)?);
                }
                Ok(self.environment_publish(out)?)
            }
            EnvironmentEnvironmentName => {
                Ok(self.allocate_text(name(n, self.text(a[0])?)?)?)
            }
            EnvironmentEnvironmentHome => {
                let values = self.environment_map(n, a[0])?;
                let home = if cfg!(windows) {
                    values
                        .get("USERPROFILE")
                        .filter(|s| !s.is_empty())
                        .cloned()
                        .or_else(|| {
                            Some(format!(
                                "{}{}",
                                values.get("HOMEDRIVE")?,
                                values.get("HOMEPATH")?
                            ))
                        })
                } else {
                    values.get("HOME").cloned()
                };
                Ok(self
                    .allocate_text(home.filter(|s| !s.is_empty()).ok_or_else(|| {
                        failed(n, "not_found", "Home directory is unavailable")
                    })?)?)
            }
            EnvironmentEnvironmentPathSeparator => {
                Ok(self.allocate_text(if cfg!(windows) { ";" } else { ":" }.into())?)
            }
            _ => Err(Error("unsupported environment native".into()).into()),
        }
    }
}
