//! Memberships use socket2's safe platform wrappers. The UDP owner retains
//! memberships; each membership refers back weakly, so native GC has no cycle.
use crate::host::{
    Error,
    {Outcome, ProviderError},
    native_network::{NetworkHandle, address, failure, host},
    native_udp::Udp,
    *,
};
use okkama_runtime_abi::Native;
use socket2::{InterfaceIndexOrAddress, SockRef};
use std::{
    cell::Cell,
    net::{IpAddr, Ipv6Addr, SocketAddr},
    rc::{Rc, Weak},
};
/// `IP_MULTICAST_IF` by interface index, as `set_multicast_if_v6` does for
/// IPv6: IPv4 multicast leaves through the interface the policy joined, not
/// the one of the routing table (#1011). socket2 sets it only by address.
fn set_multicast_if_v4(socket: &SockRef<'_>, index: u32) -> std::io::Result<()> {
    #[cfg(target_vendor = "apple")]
    {
        /// `<netinet/in.h>`: the outgoing interface by index (macOS 10.13+).
        const IP_MULTICAST_IFINDEX: libc::c_int = 66;
        let value: libc::c_uint = index;
        set_option(socket, libc::IPPROTO_IP, IP_MULTICAST_IFINDEX, &value)
    }
    #[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd", target_os = "fuchsia"))]
    {
        let unspecified = libc::in_addr { s_addr: 0 };
        let request = libc::ip_mreqn {
            imr_multiaddr: unspecified,
            imr_address: unspecified,
            imr_ifindex: libc::c_int::try_from(index).map_err(|_| std::io::ErrorKind::InvalidInput)?,
        };
        set_option(socket, libc::IPPROTO_IP, libc::IP_MULTICAST_IF, &request)
    }
    #[cfg(windows)]
    {
        // Winsock reads an address in 0.0.0.0/8 as an interface index.
        socket.set_multicast_if_v4(&std::net::Ipv4Addr::from(index))
    }
    #[cfg(not(any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_os = "fuchsia",
        windows
    )))]
    {
        let _ = (socket, index);
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

#[cfg(unix)]
fn set_option<T>(socket: &SockRef<'_>, level: libc::c_int, name: libc::c_int, value: &T) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: the descriptor is open for the life of `socket`; `value` points
    // to a live, initialised `T` of exactly the length passed, which the
    // kernel only reads.
    let status = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            level,
            name,
            (value as *const T).cast(),
            std::mem::size_of::<T>() as libc::socklen_t,
        )
    };
    if status == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

#[derive(Debug)]
pub(crate) struct Membership {
    owner: Weak<Udp>,
    group: IpAddr,
    interface: u32,
    pub(crate) closed: Cell<bool>,
}
pub(crate) fn handles(n: Native) -> bool {
    matches!(
        n,
        Native::NetworkMulticastJoin
            | Native::NetworkMulticastClose
            | Native::NetworkMulticastClosed
    )
}
impl Heap {
    pub(crate) fn native_multicast(&mut self, n: Native, a: &[Slot]) -> Result<Outcome, Error> {
        match self.multicast_execute(n, a) {
            Ok(v) => Ok(Outcome::Value(v)),
            Err(ProviderError::Engine(e)) => Err(e),
            Err(ProviderError::Native(e)) => {
                let payload = self.native_failure_new(e)?;
                self.failure_new(crate::host::FailureKind::Native(payload))
                    .map(Outcome::Failure)
            }
        }
    }
    fn multicast_execute(&mut self, n: Native, a: &[Slot]) -> Result<Slot, ProviderError> {
        use Native::*;
        let native = ProviderError::Native;
        let invalid = |code, message| native(failure(n, code, message));
        if n != NetworkMulticastJoin {
            let NetworkHandle::Membership(member) = self.network_handle(a[0])? else {
                return Err(Error("expected multicast membership".into()).into());
            };
            if n == NetworkMulticastClosed {
                return Ok(Slot::boolean(member.closed.get()));
            }
            if member.closed.replace(true) {
                return Ok(Slot::boolean(true));
            }
            if let Some(state) = member.owner.upgrade() {
                state.members.borrow_mut().remove(&member.group);
                if let Ok(socket) = state.socket(n) {
                    let socket = SockRef::from(&*socket);
                    match member.group {
                        IpAddr::V4(ip) => {
                            let _ = socket.leave_multicast_v4_n(
                                &ip,
                                &InterfaceIndexOrAddress::Index(member.interface),
                            );
                        }
                        IpAddr::V6(ip) => {
                            let _ = socket.leave_multicast_v6(&ip, member.interface);
                        }
                    }
                }
            }
            return Ok(Slot::boolean(true));
        }
        let state = self.udp(a[0])?;
        let socket = state.socket(n).map_err(native)?;
        if state.peer.get().is_some() || state.sending.get() || state.connecting.get() {
            return Err(invalid(
                "invalid_input",
                "Multicast requires an unconnected UDP socket without a pending send",
            ));
        }
        let group = address(n, &self.native_bytes(a[1])?, "").map_err(native)?;
        let name = self.text(a[2])?;
        let index = self
            .int64(a[3])?
            .filter(|n| (1..=u32::MAX as i64).contains(n));
        let hops = self
            .int64(a[4])?
            .filter(|n| (0..=255).contains(n))
            .ok_or_else(|| invalid("invalid_input", "Multicast hop limit must be in 0..255"))?
            as u32;
        let loopback = a[5].bits != 0;
        if group.port() != 0 || !group.ip().is_multicast() {
            return Err(invalid("invalid_input", "Address is not a multicast group"));
        }
        if group.is_ipv6() != state.family_v6 {
            return Err(invalid(
                "unsupported",
                "Multicast group must match socket IP family",
            ));
        }
        let valid_name = !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'))
            && !name.bytes().all(|b| b.is_ascii_digit());
        let index = index
            .filter(|_| valid_name)
            .ok_or_else(|| invalid("invalid_input", "Invalid multicast interface identity"))?
            as u32;
        if state
            .policy
            .get()
            .is_some_and(|p| p != (index, hops, loopback))
        {
            return Err(invalid(
                "invalid_input",
                "Multicast interface and options conflict with the socket policy",
            ));
        }
        if state.members.borrow().contains_key(&group.ip()) {
            return Err(invalid(
                "already_exists",
                "Multicast group is already joined on this socket",
            ));
        }
        let mut numeric = vec![79, 75, 1, 2, 0, 0];
        numeric.extend(Ipv6Addr::LOCALHOST.octets());
        let identity = address(n, &numeric, name).map_err(|_| {
            invalid(
                "invalid_input",
                "Multicast interface name/index no longer identifies the same interface",
            )
        })?;
        if !matches!(identity,SocketAddr::V6(v) if v.scope_id()==index) {
            return Err(invalid(
                "invalid_input",
                "Multicast interface name/index no longer identifies the same interface",
            ));
        }
        let socket = SockRef::from(&*socket);
        if state.policy.get().is_none() {
            let result = match group.ip() {
                IpAddr::V4(_) => set_multicast_if_v4(&socket, index)
                    .and_then(|()| socket.set_multicast_ttl_v4(hops))
                    .and_then(|()| socket.set_multicast_loop_v4(loopback)),
                IpAddr::V6(_) => socket
                    .set_multicast_if_v6(index)
                    .and_then(|()| socket.set_multicast_hops_v6(hops))
                    .and_then(|()| socket.set_multicast_loop_v6(loopback)),
            };
            result.map_err(|e| native(host(n, "Cannot configure multicast socket", e)))?;
        }
        match group.ip() {
            IpAddr::V4(ip) => {
                socket.join_multicast_v4_n(&ip, &InterfaceIndexOrAddress::Index(index))
            }
            IpAddr::V6(ip) => socket.join_multicast_v6(&ip, index),
        }
        .map_err(|e| native(host(n, "Cannot join multicast group", e)))?;
        state.policy.set(Some((index, hops, loopback)));
        let member = Rc::new(Membership {
            owner: Rc::downgrade(&state),
            group: group.ip(),
            interface: index,
            closed: Cell::new(false),
        });
        state
            .members
            .borrow_mut()
            .insert(group.ip(), member.clone());
        Ok(self.network_new(NetworkHandle::Membership(member))?)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn loopback_index() -> u32 {
        let name = if cfg!(target_vendor = "apple") { c"lo0" } else { c"lo" };
        // SAFETY: a valid NUL-terminated name; the call only reads it.
        unsafe { libc::if_nametoindex(name.as_ptr()) }
    }

    /// #1011: an IPv4 multicast socket gets the outgoing interface of its
    /// policy by index, as IPv6 does.
    #[test]
    fn ipv4_multicast_leaves_through_the_policy_interface() {
        let index = loopback_index();
        assert_ne!(index, 0);
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let socket = SockRef::from(&socket);
        set_multicast_if_v4(&socket, index).unwrap();
        #[cfg(target_vendor = "apple")]
        {
            use std::os::fd::AsRawFd;
            let mut value: libc::c_uint = 0;
            let mut length = std::mem::size_of::<libc::c_uint>() as libc::socklen_t;
            // SAFETY: `value` and `length` are live locals of the sizes passed.
            let status = unsafe {
                libc::getsockopt(socket.as_raw_fd(), libc::IPPROTO_IP, 66, (&mut value as *mut libc::c_uint).cast(), &mut length)
            };
            assert_eq!(status, 0);
            assert_eq!(value, index);
        }
        assert!(set_multicast_if_v4(&socket, u32::MAX).is_err(), "no such interface");
    }
}
