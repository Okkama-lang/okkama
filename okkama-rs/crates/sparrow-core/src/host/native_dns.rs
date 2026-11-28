//! System resolution on Tokio's blocking pool with shared admission. A started
//! libc resolver call cannot be aborted: its permit stays owned by the blocking
//! closure until actual completion. No VM data crosses the pool boundary.
use crate::host::{Error, Outcome, native_io::Output, *};
use okkama_runtime_abi::{Native, NativeFailure};
use dns_lookup::{AddrInfoHints, LookupError, LookupErrorKind};
use std::{
    collections::HashSet,
    net::{IpAddr, SocketAddr},
    sync::{Arc, OnceLock},
};

const MAX_SYSTEM_LOOKUPS: usize = 4;
static LOOKUP_SLOTS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();

struct AbortQueuedLookup(tokio::task::AbortHandle);
impl Drop for AbortQueuedLookup {
    fn drop(&mut self) {
        // Tokio can cancel queued blocking jobs, but not ones already running.
        self.0.abort();
    }
}

async fn lookup_with_slots<F, T>(
    slots: Arc<tokio::sync::Semaphore>,
    native: Native,
    lookup: F,
) -> Result<T, NativeFailure>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let permit = slots
        .acquire_owned()
        .await
        .map_err(|_| failure(native, "unexpected", "System resolver admission is closed"))?;
    let task = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        lookup()
    });
    let _abort = AbortQueuedLookup(task.abort_handle());
    task.await
        .map_err(|_| failure(native, "unexpected", "System resolver worker failed"))
}

async fn system_lookup<F, T>(native: Native, lookup: F) -> Result<T, NativeFailure>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let slots =
        LOOKUP_SLOTS.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(MAX_SYSTEM_LOOKUPS)));
    lookup_with_slots(slots.clone(), native, lookup).await
}
pub(crate) fn handles(n: Native) -> bool {
    matches!(n, Native::NetworkDnsAddresses | Native::NetworkDnsName)
}
fn failure(n: Native, code: &str, message: &str) -> NativeFailure {
    NativeFailure {
        domain: "core.net".into(),
        code: code.into(),
        operation: n.symbol().into(),
        message: message.into(),
    }
}
/// A resolver failure as the program sees it (#1011): an unknown name is
/// `not_found` (std: `UnknownHost`), a temporary failure (`EAI_AGAIN`: no
/// network, a server that did not answer) is `unavailable`, so a program can
/// retry, and anything else is `unexpected` with the resolver's own text.
fn resolver_failure(n: Native, error: &LookupError) -> NativeFailure {
    match error.kind() {
        LookupErrorKind::NoName | LookupErrorKind::NoData => failure(n, "not_found", "Network name was not found"),
        LookupErrorKind::Again => failure(n, "unavailable", "System resolver is temporarily unavailable"),
        _ => {
            let cause = std::io::Error::from(LookupError::new(error.error_num()));
            failure(n, "unexpected", &format!("System name resolution failed: {cause}"))
        }
    }
}

/// The addresses of `host` (`getaddrinfo`, one record per address and
/// stream socket, as `ToSocketAddrs` asks), at most 257 of them.
fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>, LookupError> {
    let hints = AddrInfoHints { socktype: libc::SOCK_STREAM, ..AddrInfoHints::default() };
    let records = dns_lookup::getaddrinfo(Some(host), None, Some(hints))?;
    let mut addresses = Vec::new();
    for record in records.take(257) {
        let mut address = record.map_err(LookupError::from)?.sockaddr;
        address.set_port(port);
        addresses.push(address);
    }
    Ok(addresses)
}

/// What a forward lookup asks for.
enum Forward {
    /// A numeric address answers itself.
    Numeric(Addresses),
    /// A host name, to resolve with this port.
    Name(u16),
}

/// Check a forward lookup. A numeric address (IPv6 too) answers itself before
/// the check of host name characters, which `:` would fail.
fn forward(n: Native, source: &str, port: Option<i64>) -> Result<Forward, NativeFailure> {
    let port = port
        .and_then(|n| u16::try_from(n).ok())
        .ok_or_else(|| failure(n, "invalid_input", "Invalid system resolver port"))?;
    if let Ok(address) = source.parse::<IpAddr>() {
        return records(n, vec![SocketAddr::new(address, port)]).map(Forward::Numeric);
    }
    if source.is_empty()
        || source.len() > 254
        || !source.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        return Err(failure(n, "invalid_input", "Invalid system resolver hostname"));
    }
    Ok(Forward::Name(port))
}

struct Addresses(Vec<Vec<String>>);
impl Output for Addresses {
    fn publish(self: Box<Self>, heap: &mut Heap) -> Result<Slot, crate::natives::Refusal> {
        let mut addresses = Vec::with_capacity(self.0.len());
        for row in self.0 {
            let values = row
                .into_iter()
                .map(|s| heap.allocate_text(s))
                .collect::<Result<Vec<_>, _>>()?;
            addresses.push(heap.allocate_array(values)?);
        }
        heap.allocate_array(addresses).map_err(Into::into)
    }
}
impl Output for String {
    fn publish(self: Box<Self>, heap: &mut Heap) -> Result<Slot, crate::natives::Refusal> {
        heap.allocate_text(*self).map_err(Into::into)
    }
}
fn records(n: Native, mut records: Vec<SocketAddr>) -> Result<Addresses, NativeFailure> {
    if records.len() > 256 {
        return Err(failure(
            n,
            "resource_exhausted",
            "System resolution answer exceeds 256 records",
        ));
    }
    records.sort_by_key(SocketAddr::is_ipv6);
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for record in records {
        if !seen.insert(record) {
            continue;
        }
        let (address, scope) = match record {
            SocketAddr::V4(v) => (crate::host::native_net::format(&v.ip().octets()), String::new()),
            SocketAddr::V6(v) => (
                crate::host::native_net::format(&v.ip().octets()),
                if v.scope_id() == 0 {
                    String::new()
                } else {
                    v.scope_id().to_string()
                },
            ),
        };
        result.push(vec![address, record.port().to_string(), scope]);
    }
    if result.is_empty() {
        return Err(failure(
            n,
            "not_found",
            "System name resolution returned no usable address",
        ));
    }
    Ok(Addresses(result))
}
impl Heap {
    pub(crate) fn native_dns(&mut self, n: Native, a: &[Slot]) -> Result<Outcome, Error> {
        let source = self.text(a[0])?.to_owned();
        let future = if n == Native::NetworkDnsAddresses {
            let port = self.int64(a[1])?;
            self.native_io(async move {
                let port = match forward(n, &source, port)? {
                    Forward::Numeric(addresses) => return Ok(addresses),
                    Forward::Name(port) => port,
                };
                let resolved = system_lookup(n, move || resolve(&source, port))
                    .await?
                    .map_err(|e| resolver_failure(n, &e))?;
                records(n, resolved)
            })?
        } else {
            self.native_io(async move {
                if source.is_empty()
                    || source.len() > 45
                    || !source
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.')
                {
                    return Err(failure(
                        n,
                        "invalid_input",
                        "Reverse lookup requires a bounded numeric IP address",
                    ));
                }
                let address = source.parse::<IpAddr>().map_err(|_| {
                    failure(
                        n,
                        "invalid_input",
                        "Reverse lookup requires a numeric IP address",
                    )
                })?;
                system_lookup(n, move || {
                    // The name must resolve back to the address (forward
                    // confirmation); a name that does not is not found.
                    let flags = libc::NI_NAMEREQD | libc::NI_NUMERICSERV;
                    let (name, _) = dns_lookup::getnameinfo(&SocketAddr::new(address, 0), flags)
                        .map_err(|e| resolver_failure(n, &e))?;
                    let confirmed = match resolve(&name, 0) {
                        Ok(addresses) => addresses.iter().any(|a| a.ip() == address),
                        Err(e) if matches!(e.kind(), LookupErrorKind::NoName | LookupErrorKind::NoData) => false,
                        Err(e) => return Err(resolver_failure(n, &e)),
                    };
                    if confirmed && name != address.to_string() {
                        Ok(name)
                    } else {
                        Err(failure(n, "not_found", "Network name was not found"))
                    }
                })
                .await?
            })?
        };
        Ok(Outcome::Value(future))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Waker};
    use std::time::Duration;

    const N: Native = Native::NetworkDnsAddresses;

    /// #1011: an unknown name and a temporary failure are different codes, so
    /// a program can tell them apart and retry the second; the rest keeps the
    /// resolver's text.
    #[test]
    fn resolver_failures_keep_their_kind() {
        let code = |error| resolver_failure(N, &LookupError::new(error)).code;
        assert_eq!(code(libc::EAI_NONAME), "not_found");
        assert_eq!(code(libc::EAI_AGAIN), "unavailable");
        assert_eq!(code(libc::EAI_FAIL), "unexpected");
        let other = resolver_failure(N, &LookupError::new(libc::EAI_FAIL));
        assert!(other.message.starts_with("System name resolution failed: "), "{}", other.message);
    }

    /// The hosts file answers `localhost` without a network; the port is set
    /// on every record.
    #[test]
    fn localhost_resolves_with_the_port() {
        let addresses = resolve("localhost", 42).unwrap();
        assert!(!addresses.is_empty());
        assert!(addresses.iter().all(|a| a.ip().is_loopback() && a.port() == 42), "{addresses:?}");
    }

    /// #1011: numeric addresses, IPv6 included, answer themselves; a name is
    /// checked, and so is the port.
    #[test]
    fn numeric_addresses_answer_before_the_name_check() {
        let Ok(Forward::Numeric(v6)) = forward(N, "::1", Some(42)) else { panic!("::1 is numeric") };
        assert_eq!(v6.0, vec![vec!["::1", "42", ""]]);
        let Ok(Forward::Numeric(v4)) = forward(N, "127.0.0.1", Some(7)) else { panic!("numeric") };
        assert_eq!(v4.0, vec![vec!["127.0.0.1", "7", ""]]);
        assert!(matches!(forward(N, "example.org", Some(80)), Ok(Forward::Name(80))));
        assert_eq!(forward(N, "a b", Some(80)).err().unwrap().code, "invalid_input");
        assert_eq!(forward(N, "", Some(80)).err().unwrap().code, "invalid_input");
        assert_eq!(forward(N, "example.org", Some(65536)).err().unwrap().code, "invalid_input");
        assert_eq!(forward(N, "example.org", None).err().unwrap().code, "invalid_input");
    }

    #[test]
    fn answers_preserve_family_order_scope_port_and_remove_duplicates() {
        let a = "127.0.0.1:42".parse().unwrap();
        let b = "[::1]:42".parse().unwrap();
        let scoped = SocketAddr::V6(std::net::SocketAddrV6::new("fe80::1".parse().unwrap(), 42, 0, 7));
        let result = records(N, vec![b, a, a, scoped]).unwrap();
        assert_eq!(
            result.0,
            vec![vec!["127.0.0.1", "42", ""], vec!["::1", "42", ""], vec!["fe80::1", "42", "7"]]
        );
        assert!(records(N, vec![a; 257]).is_err());
        assert!(records(N, vec![]).is_err());
    }

    #[test]
    fn cancelled_admission_never_starts_system_lookup() {
        let slots = Arc::new(tokio::sync::Semaphore::new(1));
        let held = slots.clone().try_acquire_owned().unwrap();
        let started = Arc::new(AtomicBool::new(false));
        let flag = started.clone();
        let mut lookup = Box::pin(lookup_with_slots(slots.clone(), Native::NetworkDnsName, move || {
            flag.store(true, Ordering::SeqCst);
            42
        }));
        assert!(lookup.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
        drop(lookup);
        drop(held);
        assert!(!started.load(Ordering::SeqCst));
        assert_eq!(slots.available_permits(), 1);
    }

    #[test]
    fn dropping_running_lookup_keeps_admission_until_work_finishes() {
        let runtime =
            tokio::runtime::Builder::new_current_thread().enable_all().max_blocking_threads(1).build().unwrap();
        runtime.block_on(async {
            let slots = Arc::new(tokio::sync::Semaphore::new(1));
            let (started, begin) = tokio::sync::oneshot::channel();
            let (release, wait) = std::sync::mpsc::channel::<()>();
            let mut lookup = Box::pin(lookup_with_slots(slots.clone(), Native::NetworkDnsName, move || {
                let _ = started.send(());
                // Bounded even if the test fails; no resolver or network.
                let _ = wait.recv_timeout(Duration::from_secs(1));
                42
            }));
            assert!(lookup.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
            tokio::time::timeout(Duration::from_secs(1), begin).await.unwrap().unwrap();
            drop(lookup);
            assert_eq!(slots.available_permits(), 0);
            release.send(()).unwrap();
            let permit =
                tokio::time::timeout(Duration::from_secs(1), slots.clone().acquire_owned()).await.unwrap().unwrap();
            drop(permit);
            assert_eq!(slots.available_permits(), 1);
        });
    }

    #[test]
    fn dropping_queued_lookup_prevents_its_closure_from_running() {
        let runtime =
            tokio::runtime::Builder::new_current_thread().enable_all().max_blocking_threads(1).build().unwrap();
        runtime.block_on(async {
            let (started, begin) = tokio::sync::oneshot::channel();
            let (release, wait) = std::sync::mpsc::channel::<()>();
            let blocker = tokio::task::spawn_blocking(move || {
                let _ = started.send(());
                let _ = wait.recv_timeout(Duration::from_secs(1));
            });
            tokio::time::timeout(Duration::from_secs(1), begin).await.unwrap().unwrap();
            let slots = Arc::new(tokio::sync::Semaphore::new(1));
            let ran = Arc::new(AtomicBool::new(false));
            let flag = ran.clone();
            let mut lookup = Box::pin(lookup_with_slots(slots.clone(), Native::NetworkDnsName, move || {
                flag.store(true, Ordering::SeqCst);
            }));
            assert!(lookup.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
            drop(lookup);
            release.send(()).unwrap();
            blocker.await.unwrap();
            let permit = tokio::time::timeout(Duration::from_secs(1), slots.acquire_owned()).await.unwrap().unwrap();
            drop(permit);
            assert!(!ran.load(Ordering::SeqCst));
        });
    }
}
