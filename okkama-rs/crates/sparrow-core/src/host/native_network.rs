//! TCP ownership is native Rust state; Tokio supplies readiness, connect and
//! accept. Closing a shared state wakes pending operations before dropping it.
use crate::host::{
    Error,
    handle::Handle,
    {Outcome, ProviderError},
    native_io::Output,
    *,
};
use okkama_runtime_abi::{Native, NativeFailure};
use socket2::{Domain, Protocol, SockAddr, SockRef, Socket, Type};
use std::{
    cell::{Cell, RefCell},
    future::Future,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6},
    rc::Rc,
    task::Poll,
};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
#[derive(Debug)]
pub(crate) enum Transport {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),
}
impl From<TcpStream> for Transport {
    fn from(s: TcpStream) -> Self {
        Self::Tcp(s)
    }
}
#[cfg(unix)]
impl From<tokio::net::UnixStream> for Transport {
    fn from(s: tokio::net::UnixStream) -> Self {
        Self::Unix(s)
    }
}
impl Transport {
    fn label(&self) -> &'static str {
        match self {
            Self::Tcp(_) => "TCP socket",
            #[cfg(unix)]
            Self::Unix(_) => "Unix socket",
        }
    }

    pub(crate) async fn readable(&self) -> std::io::Result<()> {
        match self {
            Self::Tcp(s) => s.readable().await,
            #[cfg(unix)]
            Self::Unix(s) => s.readable().await,
        }
    }
    pub(crate) async fn writable(&self) -> std::io::Result<()> {
        match self {
            Self::Tcp(s) => s.writable().await,
            #[cfg(unix)]
            Self::Unix(s) => s.writable().await,
        }
    }
    pub(crate) fn try_read(&self, bytes: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Tcp(s) => s.try_read(bytes),
            #[cfg(unix)]
            Self::Unix(s) => s.try_read(bytes),
        }
    }
    pub(crate) fn try_write(&self, bytes: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Tcp(s) => s.try_write(bytes),
            #[cfg(unix)]
            Self::Unix(s) => s.try_write(bytes),
        }
    }
    fn shutdown(&self, how: std::net::Shutdown) -> std::io::Result<()> {
        match self {
            Self::Tcp(s) => SockRef::from(s).shutdown(how),
            #[cfg(unix)]
            Self::Unix(s) => SockRef::from(s).shutdown(how),
        }
    }
}
#[derive(Debug)]
pub(crate) struct Stream {
    socket: RefCell<Option<Rc<Transport>>>,
    changed: tokio::sync::watch::Sender<()>,
    read_changed: tokio::sync::watch::Sender<()>,
    input_closed: Cell<bool>,
    output_closed: Cell<bool>,
    reading: Rc<Cell<bool>>,
    writing: Rc<Cell<bool>>,
}
impl Stream {
    pub(crate) fn new(socket: impl Into<Transport>) -> Self {
        Self {
            socket: RefCell::new(Some(Rc::new(socket.into()))),
            changed: tokio::sync::watch::channel(()).0,
            read_changed: tokio::sync::watch::channel(()).0,
            input_closed: Cell::new(false),
            output_closed: Cell::new(false),
            reading: Rc::new(Cell::new(false)),
            writing: Rc::new(Cell::new(false)),
        }
    }
    pub(crate) fn closed(&self) -> bool {
        self.socket.borrow().is_none()
    }
    pub(crate) fn close(&self) {
        self.socket.borrow_mut().take();
        self.changed.send_replace(());
        self.read_changed.send_replace(());
    }
    /// Hand the TCP socket over to TLS (`runtime/tls_transport.client`);
    /// the stream state is closed afterwards.
    pub(crate) fn take_for_tls(&self, n: Native) -> Result<Rc<Transport>, NativeFailure> {
        let socket = self.socket.borrow().clone().ok_or_else(|| failure(n, "already_closed", "Native handle is already closed"))?;
        let Transport::Tcp(tcp) = &*socket else {
            return Err(failure(n, "invalid_input", "TLS requires a managed TCP stream"));
        };
        if self.input_closed.get() || self.output_closed.get() {
            return Err(failure(n, "already_closed", "TLS requires both open TCP halves"));
        }
        if tcp.peer_addr().is_err() {
            return Err(failure(n, "invalid_input", "TLS requires a connected TCP stream"));
        }
        if self.reading.get() || self.writing.get() {
            return Err(failure(n, "resource_exhausted", "TLS requires drained TCP operations"));
        }
        self.close();
        Ok(socket)
    }
    pub(crate) fn socket(&self, n: Native) -> Result<Rc<Transport>, NativeFailure> {
        self.socket
            .borrow()
            .clone()
            .ok_or_else(|| failure(n, "already_closed", "Socket is closed"))
    }
}
#[derive(Debug)]
pub(crate) struct Listener {
    socket: RefCell<Option<Rc<TcpListener>>>,
    changed: tokio::sync::watch::Sender<()>,
    listening: Cell<bool>,
    accepting: Rc<Cell<bool>>,
}
impl Listener {
    fn socket(&self, n: Native) -> Result<Rc<TcpListener>, NativeFailure> {
        self.socket
            .borrow()
            .clone()
            .ok_or_else(|| failure(n, "already_closed", "Socket is closed"))
    }
    fn close(&self) {
        self.socket.borrow_mut().take();
        self.changed.send_replace(());
    }
}
#[derive(Debug)]
pub(crate) enum NetworkHandle {
    Stream(Rc<Stream>),
    Udp(Rc<crate::host::native_udp::Udp>),
    Membership(Rc<crate::host::native_multicast::Membership>),
    Listener(Rc<Listener>),
    #[cfg(unix)]
    Unix(crate::host::native_unix::UnixHandle),
}
impl Output for NetworkHandle {
    fn publish(self: Box<Self>, heap: &mut Heap) -> Result<Slot, crate::natives::Refusal> {
        heap.network_new(*self).map_err(Into::into)
    }
}
impl Output for Vec<u8> {
    fn publish(self: Box<Self>, heap: &mut Heap) -> Result<Slot, crate::natives::Refusal> {
        heap.native_byte_array(*self).map_err(Into::into)
    }
}
impl Output for bool {
    fn publish(self: Box<Self>, _: &mut Heap) -> Result<Slot, crate::natives::Refusal> {
        Ok(Slot::boolean(*self))
    }
}
/// One pending operation of a kind; released when the operation ends.
pub(crate) struct Busy(Rc<Cell<bool>>);
impl Busy {
    pub(crate) fn acquire(
        flag: &Rc<Cell<bool>>,
        n: Native,
        message: &str,
    ) -> Result<Self, NativeFailure> {
        Self::claim(flag).ok_or_else(|| failure(n, "resource_exhausted", message))
    }
    /// The flag, unless an operation of the kind is already pending.
    pub(crate) fn claim(flag: &Rc<Cell<bool>>) -> Option<Self> {
        (!flag.replace(true)).then(|| Self(flag.clone()))
    }
}
impl Drop for Busy {
    fn drop(&mut self) {
        self.0.set(false);
    }
}
struct WriteGuard {
    state: Rc<Stream>,
    complete: bool,
}
impl Drop for WriteGuard {
    fn drop(&mut self) {
        if !self.complete {
            self.state.close();
        }
    }
}
pub(crate) fn handles(n: Native) -> bool {
    use Native::*;
    matches!(
        n,
        NetworkTcpConnect
            | NetworkTcpBound
            | NetworkTcpStartListening
            | NetworkTcpListenerEndpoint
            | NetworkTcpEndpoint
            | NetworkTcpAccept
            | NetworkTcpListenerClose
            | NetworkTcpListenerClosed
            | NetworkStreamRead
            | NetworkStreamWrite
            | NetworkStreamShutdown
            | NetworkStreamHalfClosed
            | NetworkClose
            | NetworkClosed
    )
}
pub(crate) fn failure(n: Native, code: &str, message: &str) -> NativeFailure {
    NativeFailure {
        domain: "core.net".into(),
        code: code.into(),
        operation: n.symbol().into(),
        message: message.into(),
    }
}
pub(crate) fn host(n: Native, message: &str, error: std::io::Error) -> NativeFailure {
    use std::io::ErrorKind::*;
    let raw = match error.raw_os_error() {
        Some(libc::EMFILE) | Some(libc::ENFILE) => Some("resource_exhausted"),
        Some(libc::EHOSTUNREACH) | Some(libc::ENETUNREACH) => Some("network_unreachable"),
        _ => None,
    };
    let code = raw.unwrap_or(match error.kind() {
        AddrInUse => "address_in_use",
        AddrNotAvailable => "address_not_available",
        ConnectionRefused => "connection_refused",
        NetworkUnreachable | HostUnreachable => "network_unreachable",
        TimedOut => "timed_out",
        ConnectionReset | ConnectionAborted => "connection_reset",
        BrokenPipe => "broken_pipe",
        NotConnected => "not_connected",
        PermissionDenied => "permission_denied",
        OutOfMemory => "resource_exhausted",
        _ => "unexpected",
    });
    failure(n, code, message)
}
pub(crate) fn address(n: Native, bytes: &[u8], zone: &str) -> Result<SocketAddr, NativeFailure> {
    if bytes.len() < 6
        || bytes[..3] != [79, 75, 1]
        || !matches!((bytes[3], bytes.len()), (1, 10) | (2, 22))
    {
        return Err(failure(
            n,
            "invalid_data",
            "Invalid network address envelope",
        ));
    }
    let port = u16::from_be_bytes([bytes[4], bytes[5]]);
    if bytes[3] == 1 {
        if !zone.is_empty() {
            return Err(failure(n, "invalid_input", "Invalid IPv6 binding zone"));
        }
        return Ok(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(bytes[6], bytes[7], bytes[8], bytes[9])),
            port,
        ));
    }
    let ip = Ipv6Addr::from(<[u8; 16]>::try_from(&bytes[6..]).expect("validated bytes"));
    if zone.is_empty() {
        return Ok(SocketAddr::V6(SocketAddrV6::new(ip, port, 0, 0)));
    }
    if !zone
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'))
    {
        return Err(failure(n, "invalid_input", "Invalid IPv6 binding zone"));
    }
    if zone.bytes().all(|b| b.is_ascii_digit()) {
        let digits = zone.trim_start_matches('0');
        let scope = if digits.is_empty() {
            0
        } else {
            digits
                .parse::<u32>()
                .map_err(|_| failure(n, "invalid_input", "IPv6 scope index exceeds uint32"))?
        };
        return Ok(SocketAddr::V6(SocketAddrV6::new(ip, port, 0, scope)));
    }
    // An interface name gives its index; an unknown one is refused here, not
    // later by bind (macOS getaddrinfo takes any name as scope 0, #1011).
    zone_index(zone)
        .map(|scope| SocketAddr::V6(SocketAddrV6::new(ip, port, 0, scope)))
        .ok_or_else(|| {
            failure(
                n,
                "invalid_input",
                "Unknown or unsupported IPv6 interface zone",
            )
        })
}
/// The index of the interface named `name`, if there is one.
fn zone_index(name: &str) -> Option<u32> {
    let name = std::ffi::CString::new(name).ok()?;
    // SAFETY: `name` is a valid NUL-terminated string the call only reads.
    let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
    (index != 0).then_some(index)
}
pub(crate) async fn interruptible<F, T>(
    changed: tokio::sync::watch::Receiver<()>,
    n: Native,
    body: F,
) -> Result<T, NativeFailure>
where
    F: Future<Output = Result<T, NativeFailure>>,
{
    interrupted_by(changed, || failure(n, "already_closed", "Socket is closed"), body).await
}
/// Runs `body` until the state it works on is closed: a send on `changed`
/// ends it with `closed()`, the failure of the operation that was waiting.
pub(crate) async fn interrupted_by<F, T>(
    mut changed: tokio::sync::watch::Receiver<()>,
    closed: impl FnOnce() -> NativeFailure,
    body: F,
) -> Result<T, NativeFailure>
where
    F: Future<Output = Result<T, NativeFailure>>,
{
    let mut body = std::pin::pin!(body);
    let mut closing = std::pin::pin!(changed.changed());
    let mut closed = Some(closed);
    std::future::poll_fn(|cx| {
        if closing.as_mut().poll(cx).is_ready()
            && let Some(closed) = closed.take()
        {
            return Poll::Ready(Err(closed()));
        }
        body.as_mut().poll(cx)
    })
    .await
}
/// The socket of a TCP listener (`tcp_bound`): bound, not listening, so no
/// peer is queued before `tcp_start_listening` gives the queue its backlog.
/// No implicit dual stack: an IPv6 listener takes IPv4 peers only with
/// `dual` (`std/net/tcp`), whatever the system default.
fn bound_listener(endpoint: SocketAddr, reuse: bool, dual: bool) -> Result<Socket, (&'static str, std::io::Error)> {
    let socket = Socket::new(Domain::for_address(endpoint), Type::STREAM, Some(Protocol::TCP))
        .map_err(|e| ("Cannot create TCP socket", e))?;
    if endpoint.is_ipv6() {
        socket.set_only_v6(!dual).map_err(|e| ("Cannot apply TCP listener dual stack", e))?;
    }
    socket.set_reuse_address(reuse).map_err(|e| ("Cannot apply TCP listener address reuse", e))?;
    socket
        .set_nonblocking(true)
        .and_then(|()| socket.bind(&SockAddr::from(endpoint)))
        .map_err(|e| ("Cannot bind TCP listener", e))?;
    Ok(socket)
}
/// `listen` with the program's backlog; the system may round the queue up
/// or cap it (`somaxconn`), and 0 asks for its smallest.
fn start_listening(socket: SockRef<'_>, backlog: i32) -> std::io::Result<()> {
    socket.listen(backlog)
}
fn options(stream: &TcpStream, keep: bool, delay: bool, n: Native) -> Result<(), NativeFailure> {
    SockRef::from(stream)
        .set_keepalive(keep)
        .and_then(|()| stream.set_nodelay(delay))
        .map_err(|e| host(n, "Cannot apply TCP connection option", e))
}
impl Heap {
    pub(crate) fn network_new(&mut self, state: NetworkHandle) -> Result<Slot, Error> {
        Ok(Slot {
            tag: HANDLE,
            bits: self.handles.allocate(Handle::Network(state))?,
        })
    }
    pub(crate) fn network_handle(&self, slot: Slot) -> Result<&NetworkHandle, Error> {
        if slot.kind() != HANDLE {
            return Err(Error("expected network handle".into()));
        }
        match self.handles.get(slot.bits)? {
            Handle::Network(state) => Ok(state),
            _ => Err(Error("expected network handle".into())),
        }
    }
    pub(crate) fn network_stream(&self, slot: Slot) -> Result<Rc<Stream>, Error> {
        match self.network_handle(slot)? {
            NetworkHandle::Stream(state) => Ok(state.clone()),
            _ => Err(Error("expected socket handle".into())),
        }
    }
    fn network_listener(&self, slot: Slot) -> Result<Rc<Listener>, Error> {
        match self.network_handle(slot)? {
            NetworkHandle::Listener(state) => Ok(state.clone()),
            _ => Err(Error("expected listener handle".into())),
        }
    }
    pub(crate) fn network_endpoint(&mut self, endpoint: SocketAddr) -> Result<Slot, Error> {
        let (ip, scope) = match endpoint {
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
        let values = [ip, endpoint.port().to_string(), scope]
            .into_iter()
            .map(|s| self.allocate_text(s))
            .collect::<Result<Vec<_>, _>>()?;
        self.allocate_array(values)
    }
    pub(crate) fn native_network(&mut self, n: Native, a: &[Slot]) -> Result<Outcome, Error> {
        if matches!(n, Native::NetworkClose | Native::NetworkClosed)
            && let NetworkHandle::Udp(state) = self.network_handle(a[0])? {
                if n == Native::NetworkClose {
                    state.close();
                    return Ok(Outcome::Value(Slot::boolean(true)));
                }
                return Ok(Outcome::Value(Slot::boolean(state.closed())));
            }
        match self.network_execute(n, a) {
            Ok(v) => Ok(Outcome::Value(v)),
            Err(ProviderError::Engine(e)) => Err(e),
            Err(ProviderError::Native(e)) => {
                let payload = self.native_failure_new(e)?;
                let failure = self.failure_new(crate::host::FailureKind::Native(payload))?;
                if n.signature().is_async {
                    self.future_with_state(crate::host::State::Ready(Err(failure)))
                        .map(Outcome::Value)
                } else {
                    Ok(Outcome::Failure(failure))
                }
            }
        }
    }
    fn network_execute(&mut self, n: Native, a: &[Slot]) -> Result<Slot, ProviderError> {
        use Native::*;
        let invalid = |code, message| ProviderError::Native(failure(n, code, message));
        let native = ProviderError::Native;
        match n {
            NetworkTcpConnect | NetworkTcpBound => {
                let endpoint =
                    address(n, &self.native_bytes(a[0])?, self.text(a[1])?).map_err(native)?;
                let (first, second) = (a[2].bits != 0, a[3].bits != 0);
                if n == NetworkTcpConnect {
                    if endpoint.port() == 0 {
                        return Err(invalid(
                            "invalid_input",
                            "TCP destination port must be positive",
                        ));
                    }
                    return Ok(self.native_io(async move {
                        let socket = if endpoint.is_ipv4() {
                            TcpSocket::new_v4()
                        } else {
                            TcpSocket::new_v6()
                        }
                        .map_err(|e| host(n, "Cannot create TCP socket", e))?;
                        let stream = socket
                            .connect(endpoint)
                            .await
                            .map_err(|e| host(n, "TCP connection failed", e))?;
                        options(&stream, first, second, n)?;
                        Ok(NetworkHandle::Stream(Rc::new(Stream::new(stream))))
                    })?);
                }
                if endpoint.is_ipv4() && second {
                    return Err(invalid("unsupported", "Dual stack requires IPv6 listener"));
                }
                let socket = bound_listener(endpoint, first, second)
                    .map_err(|(message, e)| native(host(n, message, e)))?;
                if tokio::runtime::Handle::try_current().is_err() {
                    return Err(Error("TCP listener requires a Tokio runtime".into()).into());
                }
                let socket = TcpListener::from_std(socket.into())
                    .map_err(|e| native(host(n, "Cannot register TCP listener", e)))?;
                Ok(self.network_new(NetworkHandle::Listener(Rc::new(Listener {
                    socket: RefCell::new(Some(Rc::new(socket))),
                    changed: tokio::sync::watch::channel(()).0,
                    listening: Cell::new(false),
                    accepting: Rc::new(Cell::new(false)),
                })))?)
            }
            NetworkTcpListenerClose
            | NetworkTcpListenerClosed
            | NetworkTcpListenerEndpoint
            | NetworkTcpStartListening
            | NetworkTcpAccept => {
                let state = self.network_listener(a[0])?;
                if n == NetworkTcpListenerClose {
                    state.close();
                    return Ok(Slot::boolean(true));
                }
                if n == NetworkTcpListenerClosed {
                    return Ok(Slot::boolean(state.socket.borrow().is_none()));
                }
                let socket = state.socket(n).map_err(native)?;
                match n {
                    NetworkTcpListenerEndpoint => Ok(self.network_endpoint(
                        socket
                            .local_addr()
                            .map_err(|e| native(host(n, "Cannot inspect TCP endpoint", e)))?,
                    )?),
                    NetworkTcpStartListening => {
                        let Some(backlog) = self
                            .int64(a[1])?
                            .and_then(|v| i32::try_from(v).ok())
                            .filter(|v| *v >= 0)
                        else {
                            return Err(invalid(
                                "invalid_input",
                                "TCP backlog must be nonnegative and fit a host int",
                            ));
                        };
                        if state.listening.get() {
                            return Err(invalid(
                                "invalid_input",
                                "TCP listener is already listening",
                            ));
                        }
                        start_listening(SockRef::from(&*socket), backlog)
                            .map_err(|e| native(host(n, "Cannot listen on TCP socket", e)))?;
                        state.listening.set(true);
                        Ok(Slot::boolean(true))
                    }
                    _ => {
                        if !state.listening.get() {
                            return Err(invalid("invalid_input", "TCP socket is not listening"));
                        }
                        let busy = Busy::acquire(
                            &state.accepting,
                            n,
                            "TCP listener already has a pending accepting",
                        )
                        .map_err(native)?;
                        let changed = state.changed.subscribe();
                        let (keep, delay) = (a[1].bits != 0, a[2].bits != 0);
                        Ok(self.native_io(async move {
                            let _busy = busy;
                            let _state = state;
                            interruptible(changed, n, async move {
                                let (stream, _) = socket
                                    .accept()
                                    .await
                                    .map_err(|e| host(n, "Cannot accept TCP connection", e))?;
                                options(&stream, keep, delay, n)?;
                                Ok(NetworkHandle::Stream(Rc::new(Stream::new(stream))))
                            })
                            .await
                        })?)
                    }
                }
            }
            _ => {
                if let NetworkHandle::Udp(state) = self.network_handle(a[0])? {
                    return Err(if state.closed() {
                        invalid("already_closed", "Native handle is already closed")
                    } else {
                        invalid(
                            "invalid_input",
                            "Operation requires a managed stream connection",
                        )
                    });
                }
                let state = self.network_stream(a[0])?;
                if n == NetworkClose {
                    state.close();
                    return Ok(Slot::boolean(true));
                }
                if n == NetworkClosed {
                    return Ok(Slot::boolean(state.closed()));
                }
                let output = a.get(1).is_some_and(|v| v.bits != 0);
                if n == NetworkStreamHalfClosed {
                    return Ok(Slot::boolean(
                        state.closed()
                            || if output {
                                state.output_closed.get()
                            } else {
                                state.input_closed.get()
                            },
                    ));
                }
                if n == NetworkStreamShutdown && state.closed() {
                    return Ok(Slot::boolean(true));
                }
                let socket = state.socket(n).map_err(native)?;
                match n {
                    NetworkTcpEndpoint => {
                        let Transport::Tcp(socket) = &*socket else {
                            return Err(invalid(
                                "invalid_input",
                                "Operation requires a TCP stream",
                            ));
                        };
                        Ok(self.network_endpoint(
                            if output {
                                socket.peer_addr()
                            } else {
                                socket.local_addr()
                            }
                            .map_err(|e| native(host(n, "Cannot inspect TCP endpoint", e)))?,
                        )?)
                    }
                    NetworkStreamShutdown => {
                        let closed = if output {
                            &state.output_closed
                        } else {
                            &state.input_closed
                        };
                        if closed.get() {
                            return Ok(Slot::boolean(true));
                        }
                        if output && state.writing.get() {
                            return Err(invalid(
                                "resource_exhausted",
                                "Output shutdown requires a drained writer",
                            ));
                        }
                        socket
                            .shutdown(if output {
                                std::net::Shutdown::Write
                            } else {
                                std::net::Shutdown::Read
                            })
                            .map_err(|e| native(host(n, "Cannot shut down stream half", e)))?;
                        closed.set(true);
                        // Input shutdown must interrupt an outstanding reader only.
                        if !output {
                            state.read_changed.send_replace(());
                        }
                        Ok(Slot::boolean(true))
                    }
                    NetworkStreamRead => {
                        let limit = self.integer(a[1])?;
                        if limit.to_i64().is_some_and(|n| n <= 0)
                            || matches!(&limit,Integer::Big(v) if v.sign()!=num_bigint::Sign::Plus)
                        {
                            return Err(invalid(
                                "invalid_input",
                                "Stream read limit must be positive",
                            ));
                        }
                        if state.input_closed.get() {
                            return Err(invalid("already_closed", "Stream input is closed"));
                        }
                        let limit = limit.to_i64().map_or(65536, |v| (v as usize).min(65536));
                        let busy = Busy::acquire(
                            &state.reading,
                            n,
                            &format!("{} already has a pending reading", socket.label()),
                        )
                        .map_err(native)?;
                        let changed = state.read_changed.subscribe();
                        Ok(self.native_io(async move {
                            let _busy = busy;
                            let _state = state;
                            interruptible(changed, n, async move {
                                let mut bytes = vec![0; limit];
                                loop {
                                    socket
                                        .readable()
                                        .await
                                        .map_err(|e| host(n, "Cannot read managed stream", e))?;
                                    match socket.try_read(&mut bytes) {
                                        Ok(size) => {
                                            bytes.truncate(size);
                                            return Ok(bytes);
                                        }
                                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                            continue;
                                        }
                                        Err(e) => {
                                            return Err(host(n, "Cannot read managed stream", e));
                                        }
                                    }
                                }
                            })
                            .await
                        })?)
                    }
                    NetworkStreamWrite => {
                        if state.output_closed.get() {
                            return Err(invalid("already_closed", "Stream output is closed"));
                        }
                        let bytes = self.native_bytes(a[1])?;
                        if state.writing.get() {
                            return Err(invalid(
                                "resource_exhausted",
                                &format!("{} already has a pending writing", socket.label()),
                            ));
                        }
                        if bytes.is_empty() {
                            return Ok(self.future_with_state(crate::host::State::Ready(Ok(
                                Slot::boolean(true),
                            )))?);
                        }
                        let guard = WriteGuard {
                            state: state.clone(),
                            complete: false,
                        };
                        let busy = Busy::acquire(
                            &state.writing,
                            n,
                            &format!("{} already has a pending writing", socket.label()),
                        )
                        .map_err(native)?;
                        let changed = state.changed.subscribe();
                        Ok(self.native_io(async move {
                            let _busy = busy;
                            let mut guard = guard;
                            let result = interruptible(changed, n, async move {
                                let mut at = 0;
                                while at < bytes.len() {
                                    socket
                                        .writable()
                                        .await
                                        .map_err(|e| host(n, "Cannot write managed stream", e))?;
                                    match socket.try_write(&bytes[at..]) {
                                        Ok(0) => {
                                            return Err(failure(
                                                n,
                                                "broken_pipe",
                                                "Cannot write managed stream",
                                            ));
                                        }
                                        Ok(size) => at += size,
                                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                            continue;
                                        }
                                        Err(e) => {
                                            return Err(host(n, "Cannot write managed stream", e));
                                        }
                                    }
                                }
                                Ok(true)
                            })
                            .await;
                            guard.complete = result.is_ok();
                            result
                        })?)
                    }
                    _ => Err(Error("unsupported network provider".into()).into()),
                }
            }
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn loopback() -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0))
    }

    /// How many of `count` peers get a connection within a short wait.
    fn connected(address: SocketAddr, count: usize) -> usize {
        let peers: Vec<_> = (0..count)
            .filter_map(|_| std::net::TcpStream::connect_timeout(&address, Duration::from_millis(150)).ok())
            .collect();
        peers.len()
    }

    /// #1011: a bound listener queues no peer before it listens, and then
    /// queues only about as many as the program's backlog, not always 128.
    #[test]
    fn the_listen_queue_takes_the_program_backlog() {
        let socket = bound_listener(loopback(), true, false).unwrap();
        let address = socket.local_addr().unwrap().as_socket().unwrap();
        assert_eq!(connected(address, 1), 0, "bound but not listening refuses peers");
        start_listening(SockRef::from(&socket), 1).unwrap();
        assert!(connected(address, 1) >= 1, "a listening socket takes a peer");
        // #1169: counting queued peers is not reliable on macOS, which completed 8 of 8 handshakes for a backlog of
        // 1 under a parallel test run; its kernel reports the queue's limit instead (SO_LISTENQLIMIT).
        #[cfg(target_vendor = "apple")]
        {
            use std::os::fd::AsRawFd;
            const SO_LISTENQLIMIT: libc::c_int = 0x1100;
            let mut limit: libc::c_int = 0;
            let mut size = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
            // SAFETY: a valid descriptor and a buffer of the option's size.
            let status = unsafe {
                libc::getsockopt(socket.as_raw_fd(), libc::SOL_SOCKET, SO_LISTENQLIMIT, (&raw mut limit).cast(), &mut size)
            };
            assert_eq!(status, 0, "SO_LISTENQLIMIT");
            assert_eq!(limit, 1, "the queue's limit is the program's backlog");
        }
        // Linux queues `backlog + 1` peers; nobody accepts.
        #[cfg(not(target_vendor = "apple"))]
        {
            let queued = connected(address, 8);
            assert!((1..=3).contains(&queued), "backlog 1 queued {queued} peers");
        }

        let roomy = bound_listener(loopback(), true, false).unwrap();
        let address = roomy.local_addr().unwrap().as_socket().unwrap();
        start_listening(SockRef::from(&roomy), 128).unwrap();
        assert_eq!(connected(address, 8), 8);
    }

    /// #1011: an interface zone by name is its index; an unknown name is
    /// refused at once, on macOS too.
    #[test]
    fn a_zone_name_must_name_an_interface() {
        let loopback = if cfg!(target_vendor = "apple") { "lo0" } else { "lo" };
        let mut envelope = vec![79, 75, 1, 2, 0, 7];
        envelope.extend(Ipv6Addr::LOCALHOST.octets());
        let n = Native::NetworkTcpBound;
        let SocketAddr::V6(named) = address(n, &envelope, loopback).unwrap() else { panic!("IPv6") };
        assert_ne!(named.scope_id(), 0);
        assert_eq!(named.port(), 7);
        let unknown = address(n, &envelope, "nosuch9").unwrap_err();
        assert_eq!(unknown.message, "Unknown or unsupported IPv6 interface zone");
        assert!(matches!(address(n, &envelope, "3").unwrap(), SocketAddr::V6(v) if v.scope_id() == 3));
    }

    /// #1011: an IPv6 listener is dual stack only when asked.
    #[test]
    fn an_ipv6_listener_is_dual_stack_only_when_asked() {
        // The unspecified address: Linux makes a socket bound to a specific
        // IPv6 address (`::1`) IPv6-only whatever the option says. Bound, not
        // listening, so nothing is reachable.
        let v6 = SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0));
        let Ok(single) = bound_listener(v6, true, false) else {
            return; // no IPv6 on this host
        };
        assert!(single.only_v6().unwrap());
        assert!(!bound_listener(v6, true, true).unwrap().only_v6().unwrap());
    }
}
