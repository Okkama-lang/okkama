//! `runtime/tls_transport`: a rustls client, and a server (#209), over a TCP stream taken from
//! `runtime/network`, driven by the core's Tokio readiness like the stream
//! itself. This is the only TLS provider (runtime-core's twin went in #1016): the
//! engine checks handshake signatures but not the peer chain, which
//! `authorize` verifies explicitly (`runtime/x509.verify_chain`) before any
//! application I/O; ciphertext is fed one record at a time, so one read
//! returns the plaintext of at most one record.
//!
//! All state lives on the VM thread. Received ciphertext and plaintext go to
//! the shared buffers in the same poll that reads them, and outgoing records
//! queue in wire order, so a cancelled operation loses no bytes. A read that
//! completed but is cancelled before its result reaches the program puts
//! its plaintext back (`Received`).
use crate::host::native_network::{Busy, Transport, interrupted_by};
use crate::host::*;
use okkama_runtime_abi::{Native, NativeFailure};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{ClientConnection, Connection, DigitallySignedStruct, ServerConnection, SignatureScheme};
use std::cell::{Cell, RefCell};
use std::io::Read;
use std::rc::Rc;
use std::sync::Arc;

const CHUNK: usize = 65536;

pub(crate) fn handles(native: Native) -> bool {
    native.module() == "runtime/tls_transport"
}

fn fail(n: Native, code: &str, message: &str) -> NativeFailure {
    NativeFailure {
        domain: "core.tls_transport".into(),
        code: code.into(),
        operation: n.symbol().into(),
        message: message.into(),
    }
}

/// Peer verification is deferred to `authorize`.
#[derive(Debug)]
struct Deferred(Arc<rustls::crypto::CryptoProvider>);

impl ServerCertVerifier for Deferred {
    fn verify_server_cert(
        &self,
        _end: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// The default SunJSSE suite order (of those rustls has).
fn provider() -> rustls::crypto::CryptoProvider {
    use rustls::crypto::ring::cipher_suite::*;
    let mut provider = rustls::crypto::ring::default_provider();
    provider.cipher_suites = vec![
        TLS13_AES_256_GCM_SHA384,
        TLS13_AES_128_GCM_SHA256,
        TLS13_CHACHA20_POLY1305_SHA256,
        TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
        TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
        TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256,
        TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
        TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
        TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
    ];
    provider
}

impl From<NativeFailure> for ProviderError {
    fn from(failure: NativeFailure) -> Self {
        Self::Native(failure)
    }
}

/// One pending operation of a kind (the stream's own guard), refused with
/// this provider's failure domain.
fn busy(flag: &Rc<Cell<bool>>, n: Native, message: &str) -> Result<Busy, NativeFailure> {
    Busy::claim(flag).ok_or_else(|| fail(n, "resource_exhausted", message))
}

/// Why a TLS operation stopped.
enum Stop {
    Io,
    Truncated,
}

#[derive(Debug)]
pub(crate) struct Tls {
    connection: RefCell<Connection>,
    /// A server asks for no client certificate: its peer is settled by the handshake.
    server: bool,
    socket: RefCell<Option<Rc<Transport>>>,
    /// Ciphertext received but not yet fed to the engine.
    incoming: RefCell<Vec<u8>>,
    /// Plaintext read without a pending `read`.
    plain: RefCell<Vec<u8>>,
    /// Records produced but not yet on the wire, in order.
    outgoing: RefCell<Vec<u8>>,
    /// Fires on `close`: pending operations fail.
    changed: tokio::sync::watch::Sender<()>,
    negotiated: Cell<bool>,
    authorized: Cell<bool>,
    closing: Cell<bool>,
    eof: Cell<bool>,
    handshaking: Rc<Cell<bool>>,
    reading: Rc<Cell<bool>>,
    writing: Rc<Cell<bool>>,
    shutting: Rc<Cell<bool>>,
    peer: RefCell<Vec<Vec<u8>>>,
}

impl Tls {
    /// A state over `socket`, before the handshake.
    fn new(connection: Connection, socket: Rc<Transport>) -> Self {
        Self {
            server: matches!(connection, Connection::Server(_)),
            connection: RefCell::new(connection),
            socket: RefCell::new(Some(socket)),
            incoming: RefCell::default(),
            plain: RefCell::default(),
            outgoing: RefCell::default(),
            changed: tokio::sync::watch::channel(()).0,
            negotiated: Cell::new(false),
            authorized: Cell::new(false),
            closing: Cell::new(false),
            eof: Cell::new(false),
            handshaking: Rc::default(),
            reading: Rc::default(),
            writing: Rc::default(),
            shutting: Rc::default(),
            peer: RefCell::default(),
        }
    }

    fn closed(&self) -> bool {
        self.socket.borrow().is_none()
    }

    /// Close the state and wake every pending operation: they fail with
    /// `already_closed` instead of waiting for the peer (#1011).
    fn abort(&self) {
        self.socket.borrow_mut().take();
        self.changed.send_replace(());
    }

    fn socket(&self) -> Result<Rc<Transport>, Stop> {
        self.socket.borrow().clone().ok_or(Stop::Io)
    }

    /// Queue the engine's pending records and put the queue on the wire.
    async fn flush(&self) -> Result<(), Stop> {
        {
            let mut connection = self.connection.borrow_mut();
            let mut outgoing = self.outgoing.borrow_mut();
            while connection.wants_write() {
                connection.write_tls(&mut *outgoing).map_err(|_| Stop::Io)?;
            }
        }
        while !self.outgoing.borrow().is_empty() {
            let socket = self.socket()?;
            socket.writable().await.map_err(|_| Stop::Io)?;
            let written = match socket.try_write(&self.outgoing.borrow()) {
                Ok(0) => return Err(Stop::Io),
                Ok(size) => size,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(_) => return Err(Stop::Io),
            };
            self.outgoing.borrow_mut().drain(..written);
        }
        Ok(())
    }

    /// Feed the engine one record; false at the end of the stream.
    async fn feed_record(&self) -> Result<bool, Stop> {
        // One buffer for the whole call, allocated at the first real read.
        let mut chunk = Vec::new();
        loop {
            let record = {
                let mut incoming = self.incoming.borrow_mut();
                if incoming.len() >= 5 && !plausible_header(&incoming[..5]) {
                    // Not TLS (#1024): the engine refuses the header at once
                    // with an alert, instead of this read waiting for a body
                    // the header only seems to announce ("GET /" reads as a
                    // record of 8239 bytes).
                    Some(incoming.drain(..).collect::<Vec<u8>>())
                } else if incoming.len() >= 5 {
                    let size = 5 + usize::from(u16::from_be_bytes([incoming[3], incoming[4]]));
                    (incoming.len() >= size).then(|| incoming.drain(..size).collect::<Vec<u8>>())
                } else {
                    None
                }
            };
            if let Some(record) = record {
                let processed = {
                    let mut connection = self.connection.borrow_mut();
                    let mut slice = record.as_slice();
                    while !slice.is_empty() {
                        connection.read_tls(&mut slice).map_err(|_| Stop::Io)?;
                    }
                    connection.process_new_packets().is_ok()
                };
                if !processed {
                    // Send the alert the engine queued, then fail.
                    let _ = self.flush().await;
                    return Err(Stop::Io);
                }
                return Ok(true);
            }
            let socket = self.socket()?;
            socket.readable().await.map_err(|_| Stop::Io)?;
            chunk.resize(CHUNK, 0);
            match socket.try_read(&mut chunk) {
                Ok(0) => {
                    let mut connection = self.connection.borrow_mut();
                    let _ = connection.read_tls(&mut &[][..]);
                    let _ = connection.process_new_packets();
                    return Ok(false);
                }
                Ok(size) => self.incoming.borrow_mut().extend_from_slice(&chunk[..size]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return Err(Stop::Io),
            }
        }
    }

    async fn negotiate(&self) -> Result<(), Stop> {
        loop {
            self.flush().await?;
            if !self.connection.borrow().is_handshaking() {
                break;
            }
            if !self.feed_record().await? {
                return Err(Stop::Io);
            }
        }
        self.flush().await?;
        let mut early = Vec::new();
        let _ = self.connection.borrow_mut().reader().read_to_end(&mut early);
        self.plain.borrow_mut().extend(early);
        Ok(())
    }

    /// Plaintext that a read took but could not deliver goes back in front.
    fn return_plain(&self, bytes: Vec<u8>) {
        let mut plain = self.plain.borrow_mut();
        let rest = std::mem::take(&mut *plain);
        plain.extend(bytes);
        plain.extend(rest);
    }

    fn take_plain(&self, limit: usize) -> Option<Vec<u8>> {
        let mut plain = self.plain.borrow_mut();
        if plain.is_empty() {
            return None;
        }
        let count = plain.len().min(limit);
        Some(plain.drain(..count).collect())
    }

    /// The next plaintext; empty after the peer's `close_notify`.
    async fn receive(&self, limit: usize) -> Result<Vec<u8>, Stop> {
        if let Some(ready) = self.take_plain(limit) {
            return Ok(ready);
        }
        if self.eof.get() {
            return Ok(Vec::new());
        }
        let mut ended = false;
        let mut buffer = vec![0u8; limit];
        loop {
            {
                let mut connection = self.connection.borrow_mut();
                match connection.reader().read(&mut buffer) {
                    Ok(0) => {
                        self.eof.set(true);
                        return Ok(Vec::new());
                    }
                    Ok(count) => {
                        buffer.truncate(count);
                        // The rest of the record waits in the plaintext buffer.
                        let mut rest = Vec::new();
                        let _ = connection.reader().read_to_end(&mut rest);
                        self.plain.borrow_mut().extend(rest);
                        return Ok(buffer);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && !ended => {}
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Err(Stop::Truncated),
                    Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Err(Stop::Truncated),
                    Err(_) => return Err(Stop::Io),
                }
            }
            ended = !self.feed_record().await?;
            self.flush().await?;
        }
    }

    async fn send(&self, payload: &[u8]) -> Result<(), Stop> {
        for part in payload.chunks(16384) {
            let mut offset = 0;
            while offset < part.len() {
                let written = std::io::Write::write(&mut self.connection.borrow_mut().writer(), &part[offset..])
                    .map_err(|_| Stop::Io)?;
                if written == 0 {
                    return Err(Stop::Io);
                }
                offset += written;
                self.flush().await?;
            }
        }
        Ok(())
    }

    /// Two-way shutdown: send `close_notify`, then wait for the peer's.
    async fn shutdown(&self) -> Result<(), Stop> {
        self.connection.borrow_mut().send_close_notify();
        self.flush().await?;
        let mut sink = vec![0u8; CHUNK];
        while !self.eof.get() {
            {
                let mut connection = self.connection.borrow_mut();
                match connection.reader().read(&mut sink) {
                    Ok(0) => break,
                    Ok(_) => continue,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => break,
                }
            }
            if !self.feed_record().await? {
                break;
            }
        }
        Ok(())
    }
}

/// The plaintext of one read. A cancellation after the read completed and
/// before the program got its result drops the result unpublished: the bytes
/// then go back to the front of the plaintext buffer for the next read.
struct Received {
    bytes: Option<Vec<u8>>,
    state: Rc<Tls>,
}

impl Output for Received {
    fn publish(mut self: Box<Self>, heap: &mut Heap) -> Result<Slot, Error> {
        heap.native_byte_array(self.bytes.take().unwrap_or_default())
    }
}

impl Drop for Received {
    fn drop(&mut self) {
        if let Some(bytes) = self.bytes.take() {
            self.state.return_plain(bytes);
        }
    }
}

/// Fails the operation `n` when the state is closed under it.
async fn interruptible<T>(
    n: Native,
    changed: tokio::sync::watch::Receiver<()>,
    body: impl std::future::Future<Output = Result<T, NativeFailure>>,
) -> Result<T, NativeFailure> {
    interrupted_by(changed, || fail(n, "already_closed", "TLS owner is closed"), body).await
}

/// Whether a record header could start a TLS record (RFC 8446 §5.1, RFC 5246
/// §6.2.1): a content type from change_cipher_spec (20) to heartbeat (24), a
/// major version of 3 and at most 2^14 + 2048 bytes of body.
fn plausible_header(header: &[u8]) -> bool {
    let size = usize::from(u16::from_be_bytes([header[3], header[4]]));
    (20..=24).contains(&header[0]) && header[1] == 3 && size <= (1 << 14) + 2048
}

/// Closes the state when an operation ends otherwise than `complete`.
struct Abort {
    state: Rc<Tls>,
    complete: bool,
}

impl Drop for Abort {
    fn drop(&mut self) {
        if !self.complete {
            self.state.abort();
        }
    }
}

fn io_failure(state: &Tls, n: Native) -> NativeFailure {
    if state.closed() {
        fail(n, "already_closed", "TLS I/O readiness closed")
    } else {
        fail(n, "invalid_data", "TLS I/O failed")
    }
}

/// `SNIHostName` rules: only a host name is sent.
fn server_name(server: &str) -> Option<ServerName<'static>> {
    let sni = !server.is_empty() && !server.bytes().all(|b| b.is_ascii_digit() || b == b'.');
    if sni {
        ServerName::try_from(server.to_string()).ok()
    } else {
        Some(ServerName::IpAddress(std::net::Ipv4Addr::UNSPECIFIED.into()))
    }
}

/// OpenSSL suite names: TLS 1.3 as IANA, TLS 1.2 as `ECDHE-RSA-AES128-GCM-SHA256`.
fn openssl_cipher(iana: &str) -> String {
    let Some(rest) = iana.strip_prefix("TLS_") else {
        return iana.to_string();
    };
    let Some((exchange, rest)) = rest.split_once('_') else {
        return iana.to_string();
    };
    let Some((auth, rest)) = rest.split_once("_WITH_") else {
        return iana.to_string();
    };
    if !matches!(exchange, "ECDHE" | "DHE") || !matches!(auth, "RSA" | "ECDSA") {
        return iana.to_string();
    }
    match rest {
        "CHACHA20_POLY1305_SHA256" | "CHACHA20_POLY1305_SHA384" => format!("{exchange}-{auth}-CHACHA20-POLY1305"),
        _ => {
            for bits in ["128", "256"] {
                for hash in ["SHA256", "SHA384"] {
                    if rest == format!("AES_{bits}_GCM_{hash}") {
                        return format!("{exchange}-{auth}-AES{bits}-GCM-{hash}");
                    }
                }
            }
            iana.to_string()
        }
    }
}

impl Heap {
    pub(crate) fn native_tls(&mut self, n: Native, a: &[Slot]) -> Result<Outcome, Error> {
        let result = if n == Native::TlsTransportAuthorize { self.tls_authorize(n, a) } else { self.tls_execute(n, a).map(Outcome::Value) };
        match result {
            Ok(outcome) => Ok(outcome),
            Err(ProviderError::Engine(e)) => Err(e),
            Err(ProviderError::Native(e)) => {
                let payload = self.native_failure_new(e)?;
                let failure = self.failure_new(FailureKind::Native(payload))?;
                if n.signature().is_async {
                    self.future_with_state(State::Ready(Err(failure))).map(Outcome::Value)
                } else {
                    Ok(Outcome::Failure(failure))
                }
            }
        }
    }

    fn tls_state(&self, slot: Slot) -> Result<Rc<Tls>, Error> {
        if slot.kind() != HANDLE {
            return Err(Error("expected a TlsState".into()));
        }
        match self.handles.get(slot.bits)? {
            Handle::Tls(state) => Ok(state.clone()),
            _ => Err(Error("expected a TlsState".into())),
        }
    }

    /// The state of an operation that needs it open.
    fn tls_open(&self, n: Native, slot: Slot) -> Result<Rc<Tls>, ProviderError> {
        let state = self.tls_state(slot)?;
        if state.closed() {
            return Err(fail(n, "already_closed", "TLS owner is closed").into());
        }
        Ok(state)
    }

    /// The state of an operation that needs a verified peer.
    fn tls_verified(&self, n: Native, slot: Slot) -> Result<Rc<Tls>, ProviderError> {
        let state = self.tls_open(n, slot)?;
        if !state.authorized.get() {
            return Err(fail(n, "permission_denied", "TLS peer is not verified").into());
        }
        Ok(state)
    }

    fn tls_execute(&mut self, n: Native, a: &[Slot]) -> Result<Slot, ProviderError> {
        use Native::*;
        match n {
            TlsTransportClient => self.tls_client(n, a),
            TlsTransportServer => self.tls_server(n, a),
            TlsTransportClose => {
                let state = self.tls_state(a[0])?;
                state.abort();
                Ok(Slot::boolean(true))
            }
            TlsTransportClosed => Ok(Slot::boolean(self.tls_state(a[0])?.closed())),
            TlsTransportHandshake => {
                let state = self.tls_open(n, a[0])?;
                if state.negotiated.get() {
                    return Ok(self.future_with_state(State::Ready(Ok(Slot::boolean(true))))?);
                }
                let busy = busy(&state.handshaking, n, "TLS handshake already pending")?;
                let changed = state.changed.subscribe();
                Ok(self.native_io(async move {
                    let _busy = busy;
                    let mut guard = Abort { state: state.clone(), complete: false };
                    let result = interruptible(n, changed, async {
                        state.negotiate().await.map_err(|_| {
                            if state.closed() {
                                fail(n, "already_closed", "TLS handshake readiness closed")
                            } else {
                                fail(n, "invalid_data", "TLS handshake failed")
                            }
                        })
                    })
                    .await;
                    if result.is_ok() {
                        state.negotiated.set(true);
                        if state.server {
                            state.authorized.set(true);
                        }
                        guard.complete = true;
                    }
                    result.map(|()| true)
                })?)
            }
            TlsTransportRead => {
                let limit = self.integer(a[1])?;
                let positive = match &limit {
                    Integer::Small(v) => *v > 0,
                    Integer::Big(v) => v.sign() == num_bigint::Sign::Plus,
                };
                if !positive {
                    return Err(fail(n, "invalid_input", "TLS read limit must be positive").into());
                }
                let state = self.tls_verified(n, a[0])?;
                if state.closing.get() {
                    return Err(fail(n, "already_closed", "TLS owner is shutting down").into());
                }
                let limit = match limit {
                    Integer::Small(v) => (v as usize).min(CHUNK),
                    Integer::Big(_) => CHUNK,
                };
                let busy = busy(&state.reading, n, "TLS operation already pending")?;
                if let Some(ready) = state.take_plain(limit) {
                    let ready = self.native_byte_array(ready)?;
                    return Ok(self.future_with_state(State::Ready(Ok(ready)))?);
                }
                let changed = state.changed.subscribe();
                let keeper = state.clone();
                Ok(self.native_io(async move {
                    let _busy = busy;
                    interruptible(n, changed, async {
                        // A failed read closes the state; a cancelled one does not.
                        state.receive(limit).await.map(|bytes| Received { bytes: Some(bytes), state: keeper }).map_err(|stop| {
                            let failure = match stop {
                                Stop::Truncated => fail(n, "invalid_data", "TLS peer truncated input"),
                                Stop::Io => io_failure(&state, n),
                            };
                            state.abort();
                            failure
                        })
                    })
                    .await
                })?)
            }
            TlsTransportWrite => {
                let state = self.tls_verified(n, a[0])?;
                if state.closing.get() {
                    return Err(fail(n, "already_closed", "TLS owner is shutting down").into());
                }
                let busy = busy(&state.writing, n, "TLS operation already pending")?;
                let payload = self.byte_slice(a[1])?.into_owned();
                let changed = state.changed.subscribe();
                Ok(self.native_io(async move {
                    let _busy = busy;
                    // An unfinished write closes the state.
                    let mut guard = Abort { state: state.clone(), complete: false };
                    let result = interruptible(n, changed, async {
                        state.send(&payload).await.map_err(|_| io_failure(&state, n))
                    })
                    .await;
                    guard.complete = result.is_ok();
                    result.map(|()| true)
                })?)
            }
            TlsTransportShutdown => {
                let state = self.tls_state(a[0])?;
                if state.reading.get() || state.writing.get() {
                    return Err(fail(n, "resource_exhausted", "TLS shutdown requires drained operations").into());
                }
                let state = self.tls_verified(n, a[0])?;
                let busy = busy(&state.shutting, n, "TLS operation already pending")?;
                state.closing.set(true);
                let changed = state.changed.subscribe();
                Ok(self.native_io(async move {
                    let _busy = busy;
                    // The state closes when the shutdown ends, however it ends.
                    let _guard = Abort { state: state.clone(), complete: false };
                    interruptible(n, changed, async {
                        state.shutdown().await.map_err(|_| io_failure(&state, n))
                    })
                    .await
                    .map(|()| true)
                })?)
            }
            TlsTransportPeer => {
                let state = self.tls_verified(n, a[0])?;
                let peer = state.peer.borrow().clone();
                let items = peer.into_iter().map(|der| self.native_byte_array(der)).collect::<Result<Vec<_>, _>>()?;
                Ok(self.allocate_array(items)?)
            }
            TlsTransportVersion => {
                let state = self.tls_verified(n, a[0])?;
                let version = match state.connection.borrow().protocol_version() {
                    Some(rustls::ProtocolVersion::TLSv1_3) => "TLSv1_3",
                    Some(rustls::ProtocolVersion::TLSv1_2) => "TLSv1_2",
                    _ => "NONE",
                };
                Ok(self.allocate_text(version.into())?)
            }
            TlsTransportCipher => {
                let state = self.tls_verified(n, a[0])?;
                let suite = state
                    .connection
                    .borrow()
                    .negotiated_cipher_suite()
                    .and_then(|suite| suite.suite().as_str())
                    .unwrap_or("SSL_NULL_WITH_NULL_NULL")
                    .replacen("TLS13_", "TLS_", 1);
                Ok(self.allocate_text(openssl_cipher(&suite))?)
            }
            TlsTransportProtocol => {
                let state = self.tls_verified(n, a[0])?;
                let selected = state
                    .connection
                    .borrow()
                    .alpn_protocol()
                    .map(|bytes| bytes.iter().map(|&b| char::from(b)).collect::<String>())
                    .unwrap_or_default();
                Ok(self.allocate_text(selected)?)
            }
            _ => Err(Error("unsupported TLS provider".into()).into()),
        }
    }

    fn tls_client(&mut self, n: Native, a: &[Slot]) -> Result<Slot, ProviderError> {
        let socket = self.network_stream(a[0])?.take_for_tls(n)?;
        // Every refusal below closes the taken socket (it is dropped).
        let choice = match self.integer(a[1])? {
            Integer::Small(v) => v,
            Integer::Big(_) => 0,
        };
        if !(1..=3).contains(&choice) {
            return Err(fail(n, "invalid_input", "invalid TLS version selection").into());
        }
        let server = self.text(a[3])?.to_string();
        let mut valid = server.encode_utf16().count() <= 253
            && server.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
        let protocols = self.elements(a[2])?;
        valid &= protocols.len() <= 16;
        let mut names: Vec<String> = Vec::new();
        for protocol in protocols {
            let name = self.text(protocol)?.to_string();
            if name.is_empty()
                || name.encode_utf16().count() > 255
                || name.chars().any(|c| u32::from(c) >= 256)
                || names.contains(&name)
            {
                valid = false;
            }
            names.push(name);
        }
        if !valid {
            return Err(fail(n, "invalid_input", "invalid TLS client configuration").into());
        }
        let unavailable = || ProviderError::from(fail(n, "unsupported", "required TLS engine policy unavailable"));
        let provider = Arc::new(provider());
        let versions: &[&'static rustls::SupportedProtocolVersion] = match choice {
            1 => &[&rustls::version::TLS12],
            2 => &[&rustls::version::TLS13],
            _ => &[&rustls::version::TLS13, &rustls::version::TLS12],
        };
        let mut config = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(versions)
            .map_err(|_| unavailable())?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(Deferred(provider)))
            .with_no_client_auth();
        config.alpn_protocols = names.iter().map(|name| name.chars().map(|c| c as u8).collect()).collect();
        let name = server_name(&server).ok_or_else(unavailable)?;
        let connection = ClientConnection::new(Arc::new(config), name).map_err(|_| unavailable())?;
        let state = Tls::new(Connection::Client(connection), socket);
        Ok(Slot { tag: HANDLE, bits: self.handles.allocate(Handle::Tls(Rc::new(state)))? })
    }

    /// A TLS server over an accepted stream (#209): the certificate chain (DER,
    /// the server's own first) and a PKCS #8 key (DER); no client certificate
    /// is asked for.
    fn tls_server(&mut self, n: Native, a: &[Slot]) -> Result<Slot, ProviderError> {
        let socket = self.network_stream(a[0])?.take_for_tls(n)?;
        let choice = match self.integer(a[1])? {
            Integer::Small(v) => v,
            Integer::Big(_) => 0,
        };
        if !(1..=3).contains(&choice) {
            return Err(fail(n, "invalid_input", "invalid TLS version selection").into());
        }
        let protocols = self.elements(a[2])?;
        let mut valid = protocols.len() <= 16;
        let mut names: Vec<String> = Vec::new();
        for protocol in protocols {
            let name = self.text(protocol)?.to_string();
            if name.is_empty()
                || name.encode_utf16().count() > 255
                || name.chars().any(|c| u32::from(c) >= 256)
                || names.contains(&name)
            {
                valid = false;
            }
            names.push(name);
        }
        let mut chain: Vec<CertificateDer<'static>> = Vec::new();
        for source in self.elements(a[3])? {
            chain.push(CertificateDer::from(self.byte_slice(source)?.into_owned()));
        }
        let key = self.byte_slice(a[4])?.into_owned();
        if !valid || chain.is_empty() || chain.len() > 16 || key.is_empty() {
            return Err(fail(n, "invalid_input", "invalid TLS server configuration").into());
        }
        let unavailable = || ProviderError::from(fail(n, "unsupported", "required TLS engine policy unavailable"));
        let provider = Arc::new(provider());
        let versions: &[&'static rustls::SupportedProtocolVersion] = match choice {
            1 => &[&rustls::version::TLS12],
            2 => &[&rustls::version::TLS13],
            _ => &[&rustls::version::TLS13, &rustls::version::TLS12],
        };
        let mut config = rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(versions)
            .map_err(|_| unavailable())?
            .with_no_client_auth()
            .with_single_cert(chain, PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)))
            .map_err(|_| ProviderError::from(fail(n, "invalid_input", "invalid TLS server identity")))?;
        config.alpn_protocols = names.iter().map(|name| name.chars().map(|c| c as u8).collect()).collect();
        let connection = ServerConnection::new(Arc::new(config)).map_err(|_| unavailable())?;
        let state = Tls::new(Connection::Server(connection), socket);
        Ok(Slot { tag: HANDLE, bits: self.handles.allocate(Handle::Tls(Rc::new(state)))? })
    }

    /// Check the peer chain with `runtime/x509.verify_chain` (server purpose);
    /// a refused peer closes the state.
    fn tls_authorize(&mut self, n: Native, a: &[Slot]) -> Result<Outcome, ProviderError> {
        let state = self.tls_open(n, a[0])?;
        let result = self.tls_authorize_with(n, &state, a);
        if !matches!(result, Ok(Outcome::Value(_))) {
            state.abort();
        }
        result
    }

    fn tls_authorize_with(&mut self, n: Native, state: &Tls, a: &[Slot]) -> Result<Outcome, ProviderError> {
        let roots = self.elements(a[1])?;
        if roots.len() > 1024 {
            return Err(fail(n, "resource_exhausted", "TLS root count limit").into());
        }
        let mut total = 0usize;
        for &root in &roots {
            let size = self.byte_slice(root)?.len();
            total += size;
            if size > 1_048_576 || total > 16_777_216 {
                return Err(fail(n, "resource_exhausted", "TLS root byte limit").into());
            }
        }
        if !state.negotiated.get() {
            return Err(fail(n, "invalid_input", "TLS peer is not negotiated").into());
        }
        let sources: Vec<Vec<u8>> = state
            .connection
            .borrow()
            .peer_certificates()
            .map(|certificates| certificates.iter().map(|c| c.to_vec()).collect())
            .unwrap_or_default();
        if sources.is_empty() {
            return Err(fail(n, "invalid_data", "TLS peer certificate is absent").into());
        }
        if sources.len() > 32 {
            return Err(fail(n, "resource_exhausted", "TLS peer certificate count limit").into());
        }
        let mut sum = 0usize;
        for der in &sources {
            sum += der.len();
            if der.len() > 1_048_576 || sum > 8_388_608 {
                return Err(fail(n, "resource_exhausted", "TLS peer certificate byte limit").into());
            }
        }
        let chain = sources.iter().map(|der| self.native_byte_array(der.clone())).collect::<Result<Vec<_>, _>>()?;
        let chain = self.allocate_array(chain)?;
        let purpose = self.allocate_text("1.3.6.1.5.5.7.3.1".into())?;
        let verified = self.native_origin(Native::X509VerifyChain, &[chain, a[1], a[2], a[3], a[4], purpose])?;
        if let Outcome::Value(_) = verified {
            *state.peer.borrow_mut() = sources;
            state.authorized.set(true);
            return Ok(Outcome::Value(Slot::boolean(true)));
        }
        Ok(verified)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::time::Duration;

    /// A client state over a connected localhost stream whose peer never
    /// answers, so every read waits.
    async fn silent_client() -> (Rc<Tls>, tokio::net::TcpStream) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (client, accepted) = tokio::join!(tokio::net::TcpStream::connect(address), listener.accept());
        let provider = Arc::new(provider());
        let config = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(Deferred(provider)))
            .with_no_client_auth();
        let name = server_name("localhost").unwrap();
        let connection = ClientConnection::new(Arc::new(config), name).unwrap();
        let socket = Rc::new(Transport::from(client.unwrap()));
        (Rc::new(Tls::new(Connection::Client(connection), socket)), accepted.unwrap().0)
    }

    /// A server certificate resolver with nothing to offer: these tests end
    /// before a ClientHello is read.
    #[derive(Debug)]
    struct NoCertificate;

    impl rustls::server::ResolvesServerCert for NoCertificate {
        fn resolve(&self, _hello: rustls::server::ClientHello<'_>) -> Option<Arc<rustls::sign::CertifiedKey>> {
            None
        }
    }

    /// A server state over a connected localhost stream, and the client's end.
    async fn server_side() -> (Rc<Tls>, tokio::net::TcpStream) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (client, accepted) = tokio::join!(tokio::net::TcpStream::connect(address), listener.accept());
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(provider()))
            .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
            .unwrap()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(NoCertificate));
        let connection = ServerConnection::new(Arc::new(config)).unwrap();
        let socket = Rc::new(Transport::from(accepted.unwrap().0));
        (Rc::new(Tls::new(Connection::Server(connection), socket)), client.unwrap())
    }

    /// `peer` sends `bytes`; the handshake over `state` must fail within a
    /// second, and the peer then reads a TLS alert record (#1024).
    async fn refused_at_once(state: Rc<Tls>, mut peer: tokio::net::TcpStream, bytes: &[u8]) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        peer.write_all(bytes).await.unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(1), state.negotiate()).await;
        assert!(matches!(ended, Ok(Err(Stop::Io))), "the handshake must be refused at once");
        drop(state);
        // What the peer got, record by record (a client's ClientHello comes
        // first): one of them must be an alert (21).
        let mut received = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(1), peer.read_to_end(&mut received)).await;
        let mut types = Vec::new();
        let mut rest = received.as_slice();
        while rest.len() >= 5 {
            types.push(rest[0]);
            let size = 5 + usize::from(u16::from_be_bytes([rest[3], rest[4]]));
            rest = &rest[size.min(rest.len())..];
        }
        assert!(types.contains(&21), "the peer must get an alert, got records {types:?}");
    }

    /// #1024: plain HTTP to a TLS server is not a record header: the server
    /// refuses it with an alert at once, not after waiting for 8239 bytes.
    #[test]
    fn a_server_refuses_plain_http_at_once() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let (state, peer) = server_side().await;
            refused_at_once(state, peer, b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").await;
        });
    }

    /// #1024: a client whose server answers in plain HTTP refuses at once.
    #[test]
    fn a_client_refuses_a_plain_http_answer_at_once() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let (state, peer) = silent_client().await;
            refused_at_once(state, peer, b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n").await;
        });
    }

    /// The header check's bounds (#1024).
    #[test]
    fn a_record_header_is_checked_by_type_version_and_size() {
        let header = |kind: u8, major: u8, size: u16| {
            let [high, low] = size.to_be_bytes();
            plausible_header(&[kind, major, 3, high, low])
        };
        for kind in 20..=24 {
            assert!(header(kind, 3, 512), "content type {kind}");
        }
        assert!(!header(19, 3, 512) && !header(25, 3, 512) && !header(b'G', 3, 512));
        assert!(!header(22, 2, 512) && !header(22, 4, 512));
        assert!(header(22, 3, 18432) && !header(22, 3, 18433));
    }

    /// The check does not refuse a real record that has not arrived whole: a
    /// ClientHello's header and its first bytes keep the handshake waiting.
    #[test]
    fn a_partial_client_hello_keeps_the_handshake_waiting() {
        use tokio::io::AsyncWriteExt;
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let (state, mut peer) = server_side().await;
            peer.write_all(&[0x16, 0x03, 0x01, 0x02, 0x00, 0x01, 0x00, 0x01, 0xfc]).await.unwrap();
            let waited = tokio::time::timeout(Duration::from_millis(300), state.negotiate()).await;
            assert!(waited.is_err(), "a partial record must keep the handshake waiting");
        });
    }

    /// #1011: a close from another operation (the `Abort` guard of a failed
    /// write, a refused `authorize`) wakes a pending read at once, and the
    /// failure names the read, not `close`.
    #[test]
    fn abort_wakes_a_pending_operation_of_another_kind() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let (state, _peer) = silent_client().await;
            let reader = state.clone();
            let n = Native::TlsTransportRead;
            let mut pending = Box::pin(interruptible(n, state.changed.subscribe(), async move {
                reader.receive(16).await.map_err(|_| fail(n, "invalid_data", "TLS I/O failed"))
            }));
            let waker = std::task::Waker::noop();
            assert!(pending.as_mut().poll(&mut std::task::Context::from_waker(waker)).is_pending());
            state.abort();
            assert!(state.closed());
            let ended = tokio::time::timeout(Duration::from_secs(1), pending).await;
            let failure = ended.expect("the pending read must wake on abort").unwrap_err();
            assert_eq!(failure.code, "already_closed");
            assert_eq!(failure.operation, n.symbol());
        });
    }
}
