use crate::address::{AddressConfig, AddressInfo};
use std::io;
use std::mem;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

const NLMSG_HDRLEN: usize = mem::size_of::<libc::nlmsghdr>();
const IFADDRMSG_LEN: usize = 8;
const RTATTR_LEN: usize = 4;
const NLM_F_DUMP: u16 = 0x0300; // NLM_F_ROOT | NLM_F_MATCH
const NETLINK_RECEIVE_TIMEOUT: Duration = Duration::from_secs(5);
const NETLINK_ADD_TIMEOUT: Duration = Duration::from_secs(30);
const NETLINK_BUFFER_SIZE: usize = 16 * 1024;

static NEXT_SEQ: AtomicU32 = AtomicU32::new(1);

fn next_seq() -> u32 {
    NEXT_SEQ.fetch_add(1, Ordering::Relaxed)
}

fn align(len: usize) -> usize {
    (len + 3) & !3
}

fn invalid_data(message: &'static str) -> std::io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn push_attr(buf: &mut Vec<u8>, kind: u16, data: &[u8]) -> io::Result<()> {
    let len = RTATTR_LEN + data.len();
    if len > u16::MAX as usize {
        return Err(invalid_data("netlink attribute is too large"));
    }
    buf.extend_from_slice(&(len as u16).to_ne_bytes());
    buf.extend_from_slice(&kind.to_ne_bytes());
    buf.extend_from_slice(data);
    buf.resize(buf.len() + align(len) - len, 0);
    Ok(())
}

fn push_attr_ip(buf: &mut Vec<u8>, kind: u16, address: IpAddr) -> io::Result<()> {
    match address {
        IpAddr::V4(address) => push_attr(buf, kind, &address.octets()),
        IpAddr::V6(address) => push_attr(buf, kind, &address.octets()),
    }
}

fn address_scope(address: IpAddr) -> u8 {
    match address {
        IpAddr::V4(address) if address.is_loopback() => libc::RT_SCOPE_HOST,
        IpAddr::V4(address) if address.is_link_local() => libc::RT_SCOPE_LINK,
        IpAddr::V6(address) if address.is_unicast_link_local() => libc::RT_SCOPE_LINK,
        _ => libc::RT_SCOPE_UNIVERSE,
    }
}

struct NetlinkSocket {
    fd: OwnedFd,
    pid: u32,
}

impl NetlinkSocket {
    fn new() -> io::Result<Self> {
        // SAFETY: socket(2) returns a valid, owned descriptor on success.
        let fd = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC,
                libc::NETLINK_ROUTE,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` is a valid, owned descriptor.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };

        // SAFETY: `sockaddr_nl` is a C POD type and all-zero is valid.
        let mut local: libc::sockaddr_nl = unsafe { mem::zeroed() };
        local.nl_family = libc::AF_NETLINK as _;
        // SAFETY: `local` is a valid `sockaddr_nl` and the length is correct.
        if unsafe {
            libc::bind(
                fd.as_raw_fd(),
                &local as *const _ as *const libc::sockaddr,
                mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }

        // Strict checking lets the kernel filter RTM_GETADDR dumps by ifindex.
        // Older kernels do not support this option; if it is unavailable,
        // `ifa_index` is ignored and results are filtered in user space.
        let one: libc::c_int = 1;
        // SAFETY: `one` is a valid integer and the length is correct.
        unsafe {
            libc::setsockopt(
                fd.as_raw_fd(),
                libc::SOL_NETLINK,
                libc::NETLINK_GET_STRICT_CHK,
                &one as *const _ as *const _,
                mem::size_of::<libc::c_int>() as libc::socklen_t,
            );
        }

        // SAFETY: `sockaddr_nl` is a C POD type and all-zero is valid.
        let mut bound: libc::sockaddr_nl = unsafe { mem::zeroed() };
        let mut bound_len = mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t;
        // SAFETY: `bound` is writable and `bound_len` describes its size.
        if unsafe {
            libc::getsockname(
                fd.as_raw_fd(),
                &mut bound as *mut _ as *mut libc::sockaddr,
                &mut bound_len,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }

        Ok(Self {
            fd,
            pid: bound.nl_pid,
        })
    }

    fn send(&self, message: &[u8]) -> io::Result<()> {
        // SAFETY: `sockaddr_nl` is a C POD type and all-zero is valid.
        let mut kernel: libc::sockaddr_nl = unsafe { mem::zeroed() };
        kernel.nl_family = libc::AF_NETLINK as _;
        // SAFETY: `message` is a valid netlink request and `kernel` is a valid
        // destination address.
        let sent = unsafe {
            libc::sendto(
                self.fd.as_raw_fd(),
                message.as_ptr().cast(),
                message.len(),
                0,
                &kernel as *const _ as *const libc::sockaddr,
                mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
            )
        };
        if sent < 0 {
            return Err(io::Error::last_os_error());
        }
        if sent as usize != message.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "short netlink write",
            ));
        }
        Ok(())
    }

    fn set_receive_timeout(&self, remaining: Duration) -> io::Result<()> {
        let timeout = libc::timeval {
            tv_sec: remaining.as_secs().min(libc::time_t::MAX as u64) as libc::time_t,
            // A zero `tv_sec`/`tv_usec` disables SO_RCVTIMEO on Linux. Clamp to
            // one microsecond so a sub-microsecond remaining deadline cannot
            // turn the receive into an unbounded block.
            tv_usec: remaining.subsec_micros().max(1) as libc::suseconds_t,
        };
        // SAFETY: `timeout` is a valid `timeval` of the given length.
        if unsafe {
            libc::setsockopt(
                self.fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                &timeout as *const _ as *const _,
                mem::size_of::<libc::timeval>() as libc::socklen_t,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn receive(&self, buf: &mut [u8], deadline: Instant) -> io::Result<usize> {
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "netlink receive timed out",
                ));
            }
            self.set_receive_timeout(deadline - now)?;

            // SAFETY: `sockaddr_nl` and `msghdr` are C POD types and all-zero
            // is valid.
            let mut source: libc::sockaddr_nl = unsafe { mem::zeroed() };
            let mut iov = libc::iovec {
                iov_base: buf.as_mut_ptr().cast(),
                iov_len: buf.len(),
            };
            let mut message: libc::msghdr = unsafe { mem::zeroed() };
            message.msg_name = &mut source as *mut _ as *mut libc::c_void;
            message.msg_namelen = mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t;
            message.msg_iov = &mut iov;
            message.msg_iovlen = 1;
            // SAFETY: `buf` is writable for its full length, `iov` points to
            // it, and `source` is a writable `sockaddr_nl`.
            let received = unsafe { libc::recvmsg(self.fd.as_raw_fd(), &mut message, 0) };
            if received < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                if err.kind() == io::ErrorKind::WouldBlock {
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "netlink receive timed out",
                        ));
                    }
                    continue;
                }
                return Err(err);
            }
            // Ignore datagrams that did not come from the kernel. The operation
            // deadline still bounds how long we wait.
            if source.nl_pid != 0 {
                continue;
            }
            if message.msg_flags & libc::MSG_TRUNC != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "netlink datagram was truncated; the address dump is incomplete",
                ));
            }
            return Ok(received as usize);
        }
    }
}

fn read_header(buf: &[u8], offset: usize) -> libc::nlmsghdr {
    // SAFETY: callers check that a header fits and `read_unaligned` handles
    // arbitrary alignment of the reply buffer.
    unsafe { std::ptr::read_unaligned(buf[offset..].as_ptr() as *const libc::nlmsghdr) }
}

fn read_i32_payload(buf: &[u8], offset: usize, message_len: usize) -> io::Result<i32> {
    if message_len < NLMSG_HDRLEN + mem::size_of::<i32>() {
        return Err(invalid_data("truncated netlink response"));
    }
    // SAFETY: checked that the payload is present; `read_unaligned` handles
    // arbitrary alignment.
    Ok(unsafe { std::ptr::read_unaligned(buf[offset + NLMSG_HDRLEN..].as_ptr() as *const i32) })
}

fn netlink_error(error: i32) -> io::Result<()> {
    if error == 0 {
        return Ok(());
    }
    if error < 0 && error != i32::MIN {
        return Err(io::Error::from_raw_os_error(-error));
    }
    Err(invalid_data("invalid netlink error code"))
}

pub(crate) fn encode_newaddr(
    seq: u32,
    ifindex: u32,
    config: &AddressConfig,
) -> io::Result<Vec<u8>> {
    config
        .validate()
        .map_err(|reason| io::Error::new(io::ErrorKind::InvalidInput, reason))?;

    let family = if config.address.is_ipv4() {
        libc::AF_INET
    } else {
        libc::AF_INET6
    };
    let flags =
        (libc::NLM_F_REQUEST | libc::NLM_F_ACK | libc::NLM_F_CREATE | libc::NLM_F_EXCL) as u16;

    let mut message = Vec::with_capacity(64);

    // struct nlmsghdr
    message.extend_from_slice(&0u32.to_ne_bytes()); // nlmsg_len, patched below
    message.extend_from_slice(&libc::RTM_NEWADDR.to_ne_bytes());
    message.extend_from_slice(&flags.to_ne_bytes());
    message.extend_from_slice(&seq.to_ne_bytes());
    message.extend_from_slice(&0u32.to_ne_bytes()); // nlmsg_pid

    // struct ifaddrmsg
    message.push(family as u8);
    message.push(config.prefix_len);
    message.push(0); // ifa_flags
    message.push(address_scope(config.address));
    message.extend_from_slice(&ifindex.to_ne_bytes());

    push_attr_ip(&mut message, libc::IFA_LOCAL, config.address)?;
    push_attr_ip(
        &mut message,
        libc::IFA_ADDRESS,
        config.destination.unwrap_or(config.address),
    )?;
    if let Some(broadcast) = config.broadcast {
        push_attr(&mut message, libc::IFA_BROADCAST, &broadcast.octets())?;
    }

    let len = message.len() as u32;
    message[0..4].copy_from_slice(&len.to_ne_bytes());
    Ok(message)
}

fn encode_getaddr(seq: u32, ifindex: u32) -> Vec<u8> {
    let flags = libc::NLM_F_REQUEST as u16 | NLM_F_DUMP;

    let mut message = Vec::with_capacity(NLMSG_HDRLEN + IFADDRMSG_LEN);

    // struct nlmsghdr
    message.extend_from_slice(&0u32.to_ne_bytes()); // nlmsg_len, patched below
    message.extend_from_slice(&libc::RTM_GETADDR.to_ne_bytes());
    message.extend_from_slice(&flags.to_ne_bytes());
    message.extend_from_slice(&seq.to_ne_bytes());
    message.extend_from_slice(&0u32.to_ne_bytes()); // nlmsg_pid

    // struct ifaddrmsg
    message.push(libc::AF_UNSPEC as u8);
    message.push(0); // ifa_prefixlen
    message.push(0); // ifa_flags
    message.push(0); // ifa_scope
    message.extend_from_slice(&ifindex.to_ne_bytes());

    let len = message.len() as u32;
    message[0..4].copy_from_slice(&len.to_ne_bytes());
    message
}

#[derive(Default)]
struct Attrs<'a> {
    local: Option<&'a [u8]>,
    address: Option<&'a [u8]>,
    broadcast: Option<&'a [u8]>,
}

fn parse_attrs(attrs: &[u8]) -> io::Result<Attrs<'_>> {
    let mut result = Attrs::default();
    let mut offset = 0;
    while offset + RTATTR_LEN <= attrs.len() {
        let len = u16::from_ne_bytes([attrs[offset], attrs[offset + 1]]) as usize;
        let kind = u16::from_ne_bytes([attrs[offset + 2], attrs[offset + 3]]);
        if len < RTATTR_LEN || len > attrs.len() - offset {
            return Err(invalid_data("invalid netlink attribute length"));
        }
        let data = &attrs[offset + RTATTR_LEN..offset + len];
        match kind {
            libc::IFA_LOCAL => result.local = Some(data),
            libc::IFA_ADDRESS => result.address = Some(data),
            libc::IFA_BROADCAST => result.broadcast = Some(data),
            _ => {}
        }
        offset += align(len);
    }
    Ok(result)
}

fn parse_ip(family: u8, bytes: &[u8]) -> io::Result<IpAddr> {
    match family as i32 {
        libc::AF_INET => <[u8; 4]>::try_from(bytes)
            .map(|bytes| IpAddr::V4(Ipv4Addr::from(bytes)))
            .map_err(|_| invalid_data("invalid IPv4 address length")),
        libc::AF_INET6 => <[u8; 16]>::try_from(bytes)
            .map(|bytes| IpAddr::V6(Ipv6Addr::from(bytes)))
            .map_err(|_| invalid_data("invalid IPv6 address length")),
        _ => Err(invalid_data("unsupported address family")),
    }
}

fn parse_newaddr(message: &[u8], ifindex: u32) -> io::Result<Option<AddressInfo>> {
    if message.len() < NLMSG_HDRLEN + IFADDRMSG_LEN {
        return Err(invalid_data("truncated netlink address message"));
    }

    let ifaddr = &message[NLMSG_HDRLEN..NLMSG_HDRLEN + IFADDRMSG_LEN];
    let family = ifaddr[0];
    let prefix_len = ifaddr[1];
    let index = u32::from_ne_bytes(ifaddr[4..8].try_into().expect("fixed-size slice"));
    if index != ifindex {
        return Ok(None);
    }

    let max_prefix = match family as i32 {
        libc::AF_INET => 32,
        libc::AF_INET6 => 128,
        _ => return Ok(None),
    };
    if prefix_len > max_prefix {
        return Err(invalid_data("invalid netlink prefix length"));
    }

    let attrs = parse_attrs(&message[NLMSG_HDRLEN + IFADDRMSG_LEN..])?;
    let (address, destination) = match (attrs.local, attrs.address) {
        (Some(local), Some(peer)) if local != peer => {
            (parse_ip(family, local)?, Some(parse_ip(family, peer)?))
        }
        (Some(local), _) => (parse_ip(family, local)?, None),
        (None, Some(peer)) => (parse_ip(family, peer)?, None),
        (None, None) => return Err(invalid_data("netlink address message has no address")),
    };

    let broadcast = if family as i32 == libc::AF_INET {
        attrs
            .broadcast
            .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
            .map(Ipv4Addr::from)
    } else {
        None
    };

    Ok(Some(AddressInfo {
        address,
        prefix_len,
        destination,
        broadcast,
    }))
}

pub fn get_addresses(ifindex: u32) -> io::Result<Vec<AddressInfo>> {
    let mut last_error = None;
    for _ in 0..3 {
        match get_addresses_once(ifindex) {
            Ok(addresses) => return Ok(addresses),
            Err(err) => {
                let interrupted = err.kind() == io::ErrorKind::Interrupted;
                if !interrupted {
                    return Err(err);
                }
                last_error = Some(err);
            }
        }
    }
    Err(last_error
        .unwrap_or_else(|| invalid_data("netlink address dump was interrupted repeatedly")))
}

fn get_addresses_once(ifindex: u32) -> io::Result<Vec<AddressInfo>> {
    let socket = NetlinkSocket::new()?;
    let seq = next_seq();
    let deadline = Instant::now() + NETLINK_RECEIVE_TIMEOUT;
    socket.send(&encode_getaddr(seq, ifindex))?;

    let mut addresses = Vec::new();
    let mut reply = [0u8; NETLINK_BUFFER_SIZE];

    loop {
        let received = socket.receive(&mut reply, deadline)?;
        if received == 0 {
            return Err(invalid_data("netlink socket closed"));
        }

        let mut offset = 0;
        while offset + NLMSG_HDRLEN <= received {
            let header = read_header(&reply, offset);
            let message_len = header.nlmsg_len as usize;
            if message_len < NLMSG_HDRLEN || message_len > received - offset {
                return Err(invalid_data("invalid netlink message length"));
            }

            if header.nlmsg_pid == socket.pid && header.nlmsg_seq == seq {
                if header.nlmsg_type == libc::RTM_NEWADDR {
                    if let Some(info) =
                        parse_newaddr(&reply[offset..offset + message_len], ifindex)?
                    {
                        addresses.push(info);
                    }
                } else if header.nlmsg_type as i32 == libc::NLMSG_DONE {
                    if header.nlmsg_flags & libc::NLM_F_DUMP_INTR as u16 != 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::Interrupted,
                            "netlink address dump was interrupted",
                        ));
                    }
                    let error = read_i32_payload(&reply, offset, message_len)?;
                    netlink_error(error)?;
                    return Ok(addresses);
                } else if header.nlmsg_type as i32 == libc::NLMSG_ERROR {
                    let error = read_i32_payload(&reply, offset, message_len)?;
                    netlink_error(error)?;
                }
            }

            offset += align(message_len);
        }
    }
}

fn address_matches(config: &AddressConfig, info: &AddressInfo) -> bool {
    info.address == config.address
        && info.prefix_len == config.prefix_len
        && info.destination == config.destination
        && info.broadcast == config.broadcast
}

pub fn add_addresses(ifindex: u32, configs: &[AddressConfig]) -> io::Result<()> {
    if configs.is_empty() {
        return Ok(());
    }
    for config in configs {
        config
            .validate()
            .map_err(|reason| io::Error::new(io::ErrorKind::InvalidInput, reason))?;
    }

    let socket = NetlinkSocket::new()?;
    for config in configs {
        let seq = next_seq();
        socket.send(&encode_newaddr(seq, ifindex, config)?)?;
        // Each address gets its own full deadline: an earlier slow ack must
        // not drain the budget of the remaining addresses.
        let deadline = Instant::now() + NETLINK_ADD_TIMEOUT;
        match wait_for_ack(&socket, seq, deadline) {
            Ok(()) => {}
            Err(err) if err.raw_os_error() == Some(libc::EEXIST) => {
                // `EEXIST` only means an address with the same IP is already
                // present. Re-running an identical configuration against a
                // persistent device is idempotent, but a different prefix,
                // peer, or broadcast must not be silently ignored.
                let existing = get_addresses(ifindex)?;
                if !existing.iter().any(|info| address_matches(config, info)) {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!(
                            "address {}/{} already exists with different attributes",
                            config.address, config.prefix_len
                        ),
                    ));
                }
            }
            Err(err) => {
                return Err(io::Error::new(
                    err.kind(),
                    format!(
                        "failed to add address {}/{}: {err}",
                        config.address, config.prefix_len
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn wait_for_ack(socket: &NetlinkSocket, seq: u32, deadline: Instant) -> io::Result<()> {
    let mut reply = [0u8; 4096];
    loop {
        let received = socket.receive(&mut reply, deadline)?;
        if received == 0 {
            return Err(invalid_data("netlink socket closed"));
        }

        let mut offset = 0;
        while offset + NLMSG_HDRLEN <= received {
            let header = read_header(&reply, offset);
            let message_len = header.nlmsg_len as usize;
            if message_len < NLMSG_HDRLEN || message_len > received - offset {
                return Err(invalid_data("invalid netlink message length"));
            }

            if header.nlmsg_pid == socket.pid && header.nlmsg_seq == seq {
                if header.nlmsg_type as i32 == libc::NLMSG_ERROR {
                    let error = read_i32_payload(&reply, offset, message_len)?;
                    return netlink_error(error);
                }
                if header.nlmsg_type as i32 == libc::NLMSG_DONE {
                    return Err(invalid_data("unexpected netlink dump completion"));
                }
            }

            offset += align(message_len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn attribute<'a>(attrs: &'a [(u16, &'a [u8])], kind: u16) -> Option<&'a [u8]> {
        attrs
            .iter()
            .find_map(|(attr_type, data)| (*attr_type == kind).then_some(*data))
    }

    fn parse_test_attrs(buf: &[u8]) -> Vec<(u16, &[u8])> {
        let mut attrs = Vec::new();
        let mut offset = 0;
        while offset + RTATTR_LEN <= buf.len() {
            let len = u16::from_ne_bytes([buf[offset], buf[offset + 1]]) as usize;
            let kind = u16::from_ne_bytes([buf[offset + 2], buf[offset + 3]]);
            if len < RTATTR_LEN || len > buf.len() - offset {
                break;
            }
            attrs.push((kind, &buf[offset + RTATTR_LEN..offset + len]));
            offset += align(len);
        }
        attrs
    }

    #[test]
    fn encodes_ipv4_address_with_destination_and_broadcast() {
        let config = AddressConfig {
            address: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            prefix_len: 24,
            destination: Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))),
            broadcast: Some(Ipv4Addr::new(10, 0, 0, 255)),
        };

        let message = encode_newaddr(7, 42, &config).unwrap();
        // SAFETY: `read_unaligned` handles the byte buffer alignment.
        let header = unsafe { std::ptr::read_unaligned(message.as_ptr() as *const libc::nlmsghdr) };
        assert_eq!(header.nlmsg_len as usize, message.len());
        assert_eq!(header.nlmsg_type, libc::RTM_NEWADDR);
        assert_eq!(header.nlmsg_seq, 7);
        assert_ne!(header.nlmsg_flags & libc::NLM_F_ACK as u16, 0);

        let ifaddr = &message[NLMSG_HDRLEN..];
        assert_eq!(ifaddr[0], libc::AF_INET as u8);
        assert_eq!(ifaddr[1], 24);
        assert_eq!(u32::from_ne_bytes(ifaddr[4..8].try_into().unwrap()), 42);

        let attrs = parse_test_attrs(&message[NLMSG_HDRLEN + IFADDRMSG_LEN..]);
        assert_eq!(
            attribute(&attrs, libc::IFA_LOCAL),
            Some([10, 0, 0, 1].as_slice())
        );
        assert_eq!(
            attribute(&attrs, libc::IFA_ADDRESS),
            Some([10, 0, 0, 2].as_slice())
        );
        assert_eq!(
            attribute(&attrs, libc::IFA_BROADCAST),
            Some([10, 0, 0, 255].as_slice())
        );
    }

    #[test]
    fn encodes_ipv6_address_without_broadcast() {
        let address: Ipv6Addr = "fd00::1".parse().unwrap();
        let config = AddressConfig {
            address: IpAddr::V6(address),
            prefix_len: 64,
            destination: None,
            broadcast: None,
        };

        let message = encode_newaddr(1, 9, &config).unwrap();
        // SAFETY: `read_unaligned` handles the byte buffer alignment.
        let header = unsafe { std::ptr::read_unaligned(message.as_ptr() as *const libc::nlmsghdr) };
        assert_eq!(header.nlmsg_type, libc::RTM_NEWADDR);
        assert_eq!(header.nlmsg_seq, 1);

        let ifaddr = &message[NLMSG_HDRLEN..];
        assert_eq!(ifaddr[0], libc::AF_INET6 as u8);
        assert_eq!(ifaddr[1], 64);
        assert_eq!(u32::from_ne_bytes(ifaddr[4..8].try_into().unwrap()), 9);

        let attrs = parse_test_attrs(&message[NLMSG_HDRLEN + IFADDRMSG_LEN..]);
        assert_eq!(
            attribute(&attrs, libc::IFA_LOCAL),
            Some(address.octets().as_slice())
        );
        assert_eq!(
            attribute(&attrs, libc::IFA_ADDRESS),
            Some(address.octets().as_slice())
        );
        assert!(attribute(&attrs, libc::IFA_BROADCAST).is_none());
    }

    #[test]
    fn parses_ipv4_address_with_peer_and_broadcast() {
        let config = AddressConfig {
            address: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            prefix_len: 24,
            destination: Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))),
            broadcast: Some(Ipv4Addr::new(10, 0, 0, 255)),
        };
        let message = encode_newaddr(1, 42, &config).unwrap();

        let info = parse_newaddr(&message, 42).unwrap().expect("address");
        assert_eq!(info.address, config.address);
        assert_eq!(info.prefix_len, 24);
        assert_eq!(info.destination, config.destination);
        assert_eq!(info.broadcast, config.broadcast);
        assert!(parse_newaddr(&message, 7).unwrap().is_none());
    }

    #[test]
    fn parses_ipv6_address_with_peer() {
        let config = AddressConfig {
            address: IpAddr::V6("fd00::1".parse().unwrap()),
            prefix_len: 64,
            destination: Some(IpAddr::V6("fd00::2".parse().unwrap())),
            broadcast: None,
        };
        let message = encode_newaddr(1, 42, &config).unwrap();

        let info = parse_newaddr(&message, 42).unwrap().expect("address");
        assert_eq!(info.address, config.address);
        assert_eq!(info.prefix_len, 64);
        assert_eq!(info.destination, config.destination);
        assert_eq!(info.broadcast, None);
    }

    #[test]
    fn rejects_invalid_prefix_in_reply() {
        let config = AddressConfig {
            address: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            prefix_len: 24,
            destination: None,
            broadcast: None,
        };
        let mut message = encode_newaddr(1, 42, &config).unwrap();
        message[NLMSG_HDRLEN + 1] = 33;

        assert!(parse_newaddr(&message, 42).is_err());
    }

    #[test]
    fn rejects_malformed_attributes() {
        let malformed = [0u8; RTATTR_LEN];
        assert!(parse_attrs(&malformed).is_err());
    }

    #[test]
    fn address_match_compares_prefix_peer_and_broadcast() {
        let config = AddressConfig {
            address: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            prefix_len: 24,
            destination: Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))),
            broadcast: Some(Ipv4Addr::new(10, 0, 0, 255)),
        };
        let mut info = AddressInfo {
            address: config.address,
            prefix_len: config.prefix_len,
            destination: config.destination,
            broadcast: config.broadcast,
        };
        assert!(address_matches(&config, &info));

        info.prefix_len = 25;
        assert!(!address_matches(&config, &info));
        info.prefix_len = config.prefix_len;

        info.destination = None;
        assert!(!address_matches(&config, &info));
        info.destination = config.destination;

        info.broadcast = None;
        assert!(!address_matches(&config, &info));
    }

    #[test]
    fn dumps_loopback_v4_and_v6_addresses() {
        let name = std::ffi::CString::new("lo").unwrap();
        // SAFETY: `name` is a valid NUL-terminated string.
        // SAFETY: `name` is NUL-terminated and valid for the call.
        let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
        assert_ne!(index, 0);

        let addresses = get_addresses(index).unwrap();
        assert!(addresses.iter().any(|info| info.address.is_ipv4()));
        // IPv6 can be disabled in some environments.
        if addresses.iter().any(|info| info.address.is_ipv6()) {
            assert!(addresses.iter().any(|info| {
                info.address == IpAddr::V6(Ipv6Addr::LOCALHOST) && info.prefix_len == 128
            }));
        }
    }
}
