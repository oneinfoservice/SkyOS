//! Networking operations and socket management.

use crate::errno::Error;
use crate::io;
use alloc::vec::Vec;

/// Socket domain.
#[repr(u64)]
pub enum SocketDomain {
    /// Unix domain sockets (socketpair).
    Unix = 1,
    /// IPv4 internet protocols.
    Inet = 2,
    /// IPv6 internet protocols.
    Inet6 = 10,
}

/// Socket type.
#[repr(u64)]
pub enum SocketType {
    /// Reliable, connection-oriented byte streams.
    Stream = 1,
    /// Connectionless, unreliable datagrams.
    Datagram = 2,
    /// Raw network protocol access.
    Raw = 3,
}

/// Poll event flags.
pub const POLLIN: i16 = 0x0001;
/// Poll event flags.
pub const POLLOUT: i16 = 0x0004;
/// Poll event flags.
pub const POLLERR: i16 = 0x0008;

/// Poll file descriptor structure.
#[repr(C)]
pub struct PollFd {
    pub fd: i64,
    pub events: i16,
    pub revents: i16,
}

/// Waits for events on multiple file descriptors.
pub fn poll(fds: &mut [PollFd], timeout_ms: i32) -> Result<i32, Error> {
    // SAFETY: poll syscall is safe here
    let r = unsafe {
        crate::syscall::syscall3(
            7,
            fds.as_mut_ptr() as u64,
            fds.len() as u64,
            timeout_ms as u64,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r as i32)
    }
}

/// IPv4 Socket Address structure.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SockAddrIn {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: [u8; 4],
    pub sin_zero: [u8; 8],
}

impl SockAddrIn {
    pub fn new(ip: [u8; 4], port: u16) -> Self {
        Self {
            sin_family: 2, // AF_INET
            sin_port: port.to_be(),
            sin_addr: ip,
            sin_zero: [0; 8],
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        unsafe {
            core::slice::from_raw_parts(self as *const _ as *const u8, core::mem::size_of::<Self>())
        }
    }
}

/// IPv6 Socket Address structure (sockaddr_in6).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SockAddrIn6 {
    pub sin6_family: u16,    // AF_INET6 = 10
    pub sin6_port: u16,      // port in network byte order
    pub sin6_flowinfo: u32,  // IPv6 flow information
    pub sin6_addr: [u8; 16], // IPv6 address
    pub sin6_scope_id: u32,  // Scope ID
}

impl SockAddrIn6 {
    pub fn new(ip: [u8; 16], port: u16) -> Self {
        Self {
            sin6_family: 10, // AF_INET6
            sin6_port: port.to_be(),
            sin6_flowinfo: 0,
            sin6_addr: ip,
            sin6_scope_id: 0,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        unsafe {
            core::slice::from_raw_parts(self as *const _ as *const u8, core::mem::size_of::<Self>())
        }
    }
}

/// Opace socket address storage large enough for sockaddr_in or sockaddr_in6.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SockAddrStorage {
    pub bytes: [u8; 32],
}

impl SockAddrStorage {
    pub fn as_in(&self) -> Option<&SockAddrIn> {
        if self.bytes[0..2] == [2, 0] {
            Some(unsafe { &*(self as *const _ as *const SockAddrIn) })
        } else {
            None
        }
    }

    pub fn as_in6(&self) -> Option<&SockAddrIn6> {
        if self.bytes[0..2] == [10, 0] {
            Some(unsafe { &*(self as *const _ as *const SockAddrIn6) })
        } else {
            None
        }
    }

    pub fn as_mut_in(&mut self) -> Option<&mut SockAddrIn> {
        if self.bytes[0..2] == [2, 0] {
            Some(unsafe { &mut *(self as *mut _ as *mut SockAddrIn) })
        } else {
            None
        }
    }

    pub fn as_mut_in6(&mut self) -> Option<&mut SockAddrIn6> {
        if self.bytes[0..2] == [10, 0] {
            Some(unsafe { &mut *(self as *mut _ as *mut SockAddrIn6) })
        } else {
            None
        }
    }
}

/// Parses an IPv6 address from a string (e.g. "fe80::1").
pub fn parse_ipv6(s: &str) -> Option<[u8; 16]> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() < 2 || parts.len() > 8 {
        return None;
    }

    // prefix before ::
    let mut idx = 0;
    let mut prefix = [0u16; 8];
    while idx < parts.len() && !parts[idx].is_empty() {
        prefix[idx] = u16::from_str_radix(parts[idx], 16).ok()?;
        idx += 1;
    }
    let p_len = idx;
    // consume :: empty slots
    while idx < parts.len() && parts[idx].is_empty() {
        idx += 1;
    }
    let has_dc = idx > p_len;
    // `::` compresses exactly ONE run of empty groups; split(':') renders that
    // as 1 empty part (interior `::`), 2 empty parts (boundary `::`, e.g.
    // "::1" or "1::"), or 3 empty parts covering the whole string (the bare
    // "::" all-zero address). Anything else -- ":::1", "a:::b", ":", ":::"
    // -- is malformed and must not parse.
    let run_len = idx - p_len;
    let run_covers_all = p_len == 0 && idx == parts.len();
    // A 2-empty run is only the split artifact of a boundary `::` ("::1",
    // "1::"); interior "1:::2" is malformed.
    let valid_dc = !has_dc
        || run_len == 1
        || (run_len == 2 && (p_len == 0 || idx == parts.len()) && !run_covers_all)
        || (run_len == 3 && run_covers_all);
    if !valid_dc {
        return None;
    }
    // suffix after ::
    let mut suffix = [0u16; 8];
    let mut s_len = 0;
    while idx < parts.len() {
        suffix[s_len] = u16::from_str_radix(parts[idx], 16).ok()?;
        s_len += 1;
        idx += 1;
    }
    if p_len + s_len > 8 || (!has_dc && p_len + s_len != 8) {
        return None;
    }

    let mut out = [0u16; 8];
    out[..p_len].copy_from_slice(&prefix[..p_len]);
    for i in 0..s_len {
        out[8 - s_len + i] = suffix[i];
    }
    // ponytail: fixed-endian, no scope-id parsing

    let mut addr = [0u8; 16];
    for i in 0..8 {
        addr[i * 2] = (out[i] >> 8) as u8;
        addr[i * 2 + 1] = out[i] as u8;
    }
    Some(addr)
}

/// Resolves a hostname to an IPv4 address.
pub fn resolve(name: &str, out_ip: &mut [u8; 4]) -> Result<(), Error> {
    let mut buf = [0u8; 256];
    let bytes = name.as_bytes();
    let len = bytes.len().min(255);
    buf[..len].copy_from_slice(&bytes[..len]);
    buf[len] = 0;
    // SAFETY: resolve syscall is safe here
    let r =
        unsafe { crate::syscall::syscall2(200, buf.as_ptr() as u64, out_ip.as_mut_ptr() as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// A network socket.
pub struct Socket {
    fd: i64,
}

impl Socket {
    /// Creates a new socket.
    pub fn new(domain: SocketDomain, stype: SocketType, protocol: i32) -> Result<Self, Error> {
        // SAFETY: socket syscall is safe here
        let r =
            unsafe { crate::syscall::syscall3(41, domain as u64, stype as u64, protocol as u64) };
        if r < 0 {
            Err(Error::from_i64(r))
        } else {
            Ok(Socket { fd: r })
        }
    }

    /// Binds the socket to a local address.
    pub fn bind(&self, addr: &[u8]) -> Result<(), Error> {
        // SAFETY: bind syscall is safe here
        let r = unsafe {
            crate::syscall::syscall3(49, self.fd as u64, addr.as_ptr() as u64, addr.len() as u64)
        };
        if r < 0 {
            Err(Error::from_i64(r))
        } else {
            Ok(())
        }
    }

    /// Listens for incoming connections.
    pub fn listen(&self, backlog: i32) -> Result<(), Error> {
        // SAFETY: listen syscall is safe here
        let r = unsafe { crate::syscall::syscall2(50, self.fd as u64, backlog as u64) };
        if r < 0 {
            Err(Error::from_i64(r))
        } else {
            Ok(())
        }
    }

    /// Accepts a new connection on the socket.
    pub fn accept(&self, addr: &mut [u8], addrlen: &mut u32) -> Result<Socket, Error> {
        // SAFETY: accept syscall is safe here
        let r = unsafe {
            crate::syscall::syscall3(
                43,
                self.fd as u64,
                addr.as_mut_ptr() as u64,
                addrlen as *mut u32 as u64,
            )
        };
        if r < 0 {
            Err(Error::from_i64(r))
        } else {
            Ok(Socket { fd: r })
        }
    }

    /// Connects the socket to a remote address.
    pub fn connect(&self, addr: &[u8]) -> Result<(), Error> {
        // SAFETY: connect syscall is safe here
        let r = unsafe {
            crate::syscall::syscall3(42, self.fd as u64, addr.as_ptr() as u64, addr.len() as u64)
        };
        if r < 0 {
            Err(Error::from_i64(r))
        } else {
            Ok(())
        }
    }

    /// Reads from the socket.
    pub fn read(&self, buf: &mut [u8]) -> Result<usize, Error> {
        io::read(self.fd, buf)
    }

    /// Writes to the socket.
    pub fn write(&self, buf: &[u8]) -> Result<usize, Error> {
        io::write(self.fd, buf)
    }

    /// Returns the underlying file descriptor.
    pub fn as_raw_fd(&self) -> i64 {
        self.fd
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        let _ = io::close(self.fd);
    }
}

/// Convenience functions for sockets.
pub fn socket(domain: u64, stype: u64, protocol: i32) -> Result<i64, Error> {
    let r = unsafe { crate::syscall::syscall3(41, domain, stype, protocol as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r)
    }
}

/// Binds a raw file descriptor to an address.
pub fn bind(fd: i64, addr: &[u8]) -> Result<(), Error> {
    let r =
        unsafe { crate::syscall::syscall3(49, fd as u64, addr.as_ptr() as u64, addr.len() as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Listens on a raw file descriptor.
pub fn listen(fd: i64, backlog: i32) -> Result<(), Error> {
    let r = unsafe { crate::syscall::syscall2(50, fd as u64, backlog as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Accepts a connection on a raw file descriptor.
pub fn accept(fd: i64, addr: &mut [u8], addrlen: &mut u32) -> Result<i64, Error> {
    let r = unsafe {
        crate::syscall::syscall3(
            43,
            fd as u64,
            addr.as_mut_ptr() as u64,
            addrlen as *mut u32 as u64,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r)
    }
}

/// Connects a raw file descriptor to an address.
pub fn connect(fd: i64, addr: &[u8]) -> Result<(), Error> {
    let r =
        unsafe { crate::syscall::syscall3(42, fd as u64, addr.as_ptr() as u64, addr.len() as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Sends data through a raw socket file descriptor.
pub fn send(fd: i64, buf: &[u8]) -> Result<usize, Error> {
    io::write(fd, buf)
}

/// Receives data from a raw socket file descriptor.
pub fn recv(fd: i64, buf: &mut [u8]) -> Result<usize, Error> {
    io::read(fd, buf)
}

/// Legacy constants.
pub const AF_INET: u64 = 2;
/// Legacy constants.
pub const AF_INET6: u64 = 10;
/// Legacy constants.
pub const SOCK_STREAM: u64 = 1;

/// Message header for sendmsg/recvmsg.
#[repr(C)]
pub struct MsgHdr {
    pub msg_name: *mut u8,
    pub msg_namelen: u32,
    pub msg_iov: *const IoVec,
    pub msg_iovlen: usize,
    pub msg_control: *mut u8,
    pub msg_controllen: usize,
    pub msg_flags: i32,
}

#[repr(C)]
pub struct IoVec {
    pub iov_base: *mut u8,
    pub iov_len: usize,
}

/// Send a message on a socket.
pub fn sendmsg(sockfd: i64, msg: &MsgHdr, flags: i32) -> Result<usize, Error> {
    let r = unsafe {
        crate::syscall::syscall3(46, sockfd as u64, msg as *const MsgHdr as u64, flags as u64)
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r as usize)
    }
}

/// Receive a message on a socket.
pub fn recvmsg(sockfd: i64, msg: &mut MsgHdr, flags: i32) -> Result<usize, Error> {
    let r = unsafe {
        crate::syscall::syscall3(47, sockfd as u64, msg as *mut MsgHdr as u64, flags as u64)
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(r as usize)
    }
}

/// Get socket name (local address).
pub fn getsockname(sockfd: i64, addr: &mut [u8], addrlen: &mut u32) -> Result<(), Error> {
    let r = unsafe {
        crate::syscall::syscall3(
            51,
            sockfd as u64,
            addr.as_mut_ptr() as u64,
            addrlen as *mut u32 as u64,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Get peer name (remote address).
pub fn getpeername(sockfd: i64, addr: &mut [u8], addrlen: &mut u32) -> Result<(), Error> {
    let r = unsafe {
        crate::syscall::syscall3(
            52,
            sockfd as u64,
            addr.as_mut_ptr() as u64,
            addrlen as *mut u32 as u64,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Get a socket option.
pub fn getsockopt(
    sockfd: i64,
    level: i32,
    optname: i32,
    optval: &mut [u8],
    optlen: &mut u32,
) -> Result<(), Error> {
    let r = unsafe {
        crate::syscall::syscall5(
            55,
            sockfd as u64,
            level as u64,
            optname as u64,
            optval.as_mut_ptr() as u64,
            optlen as *mut u32 as u64,
        )
    };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok(())
    }
}

/// Create a pair of connected sockets.
pub fn socketpair(domain: u64, type_: u64, protocol: u64) -> Result<(i64, i64), Error> {
    let mut sv = [0i32; 2];
    let r =
        unsafe { crate::syscall::syscall4(53, domain, type_, protocol, sv.as_mut_ptr() as u64) };
    if r < 0 {
        Err(Error::from_i64(r))
    } else {
        Ok((sv[0] as i64, sv[1] as i64))
    }
}

/// Parses an IPv4 address from a string.
pub fn parse_ipv4(ip: &str) -> Option<u32> {
    let parts: Vec<&str> = ip.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut addr = 0u32;
    for (i, part) in parts.iter().enumerate() {
        let val: u8 = part.parse().ok()?;
        addr |= (val as u32) << (i * 8);
    }
    Some(addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ipv4_loopback() {
        let ip = parse_ipv4("127.0.0.1");
        assert_eq!(ip, Some(0x0100007f));
    }

    #[test]
    fn test_parse_ipv4_google_dns() {
        let ip = parse_ipv4("8.8.8.8");
        assert_eq!(ip, Some(0x08080808));
    }

    #[test]
    fn test_parse_ipv4_invalid_format() {
        assert_eq!(parse_ipv4(""), None);
        assert_eq!(parse_ipv4("not.an.ip"), None);
        assert_eq!(parse_ipv4("1.2.3.256"), None);
        assert_eq!(parse_ipv4("1.2.3"), None);
        assert_eq!(parse_ipv4("1.2.3.4.5"), None);
        assert_eq!(parse_ipv4("a.b.c.d"), None);
    }

    #[test]
    fn test_parse_ipv4_localhost() {
        let ip = parse_ipv4("0.0.0.0");
        assert_eq!(ip, Some(0));
    }

    #[test]
    fn test_parse_ipv4_broadcast() {
        let ip = parse_ipv4("255.255.255.255");
        assert_eq!(ip, Some(0xffffffff));
    }

    #[test]
    fn test_sockaddr_in_new() {
        let addr = SockAddrIn::new([192, 168, 1, 1], 8080);
        assert_eq!(addr.sin_family, 2);
        assert_eq!(addr.sin_port, 8080u16.to_be());
        assert_eq!(addr.sin_addr, [192, 168, 1, 1]);
        assert_eq!(addr.sin_zero, [0; 8]);
    }

    #[test]
    fn test_sockaddr_in6_new() {
        let addr = SockAddrIn6::new([0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1], 443);
        assert_eq!(addr.sin6_family, 10);
        assert_eq!(addr.sin6_port, 443u16.to_be());
        assert_eq!(addr.sin6_flowinfo, 0);
        assert_eq!(addr.sin6_scope_id, 0);
    }

    #[test]
    fn test_parse_ipv6_loopback() {
        let ip = parse_ipv6("::1");
        let mut expected = [0u8; 16];
        expected[15] = 1;
        assert_eq!(ip, Some(expected));
    }

    #[test]
    fn test_parse_ipv6_full() {
        let ip = parse_ipv6("2001:db8:85a3:0:0:8a2e:370:7334");
        assert!(ip.is_some());
        let a = ip.unwrap();
        assert_eq!(a[0..2], [0x20, 0x01]);
        assert_eq!(a[2..4], [0x0d, 0xb8]);
        // "7334" parses as 0x7334, i.e. bytes [0x73, 0x34]
        assert_eq!(a[14..16], [0x73, 0x34]);
    }

    #[test]
    fn test_parse_ipv6_invalid() {
        assert_eq!(parse_ipv6(""), None);
        assert_eq!(parse_ipv6("not:valid"), None);
        assert_eq!(parse_ipv6(":::1"), None);
        assert_eq!(parse_ipv6(":::"), None);
        assert_eq!(parse_ipv6(":"), None);
        assert_eq!(parse_ipv6("1:::2"), None);
        assert_eq!(parse_ipv6("1::2::3"), None);
        assert_eq!(parse_ipv6("1:2:3:4:5:6:7"), None);
    }

    #[test]
    fn test_parse_ipv6_double_colon_boundaries() {
        // The all-zero address written as the bare "::".
        assert_eq!(parse_ipv6("::"), Some([0u8; 16]));
        // Boundary :: on the right: 1:0:0:0:0:0:0:0 (group 1 leads)
        let mut expected = [0u8; 16];
        expected[0] = 0x00;
        expected[1] = 1;
        assert_eq!(parse_ipv6("1::"), Some(expected));
        // Interior :: with groups on both sides.
        let ip = parse_ipv6("1::2");
        let a = ip.unwrap();
        assert_eq!(a[0..2], [0, 1]);
        assert_eq!(a[14..16], [0, 2]);
    }

    #[test]
    fn test_parse_ipv6_link_local() {
        let ip = parse_ipv6("fe80::1");
        assert!(ip.is_some());
        let a = ip.unwrap();
        assert_eq!(a[0], 0xfe);
        assert_eq!(a[1], 0x80);
        assert_eq!(a[15], 1);
    }

    #[test]
    fn test_sockaddr_storage_as_in() {
        let mut bytes = [0u8; 32];
        bytes[0] = 2; // AF_INET
        bytes[2..4].copy_from_slice(&0x1f90u16.to_be_bytes());
        bytes[4..8].copy_from_slice(&[192, 168, 1, 1]);
        let storage = SockAddrStorage { bytes };
        let addr = storage.as_in();
        assert!(addr.is_some());
        assert_eq!(addr.unwrap().sin_addr, [192, 168, 1, 1]);
    }

    #[test]
    fn test_sockaddr_storage_as_in6() {
        let mut bytes = [0u8; 32];
        bytes[0] = 10;
        bytes[1] = 0;
        bytes[2] = 0x01;
        bytes[3] = 0xbb;
        let storage = SockAddrStorage { bytes };
        let addr = storage.as_in6();
        assert!(addr.is_some());
        assert_eq!(addr.unwrap().sin6_port, 443u16.to_be());
    }
}

// HttpClient is defined in libsarga/src/libskyos/net_ext.rs or elsewhere if needed.
// Duplicate removed here to avoid compilation issues.

/// Simple HTTP client for making HTTP GET requests
pub struct HttpClient {
    socket: Socket,
}

impl HttpClient {
    /// Creates a new HTTP client
    pub fn new() -> Result<Self, Error> {
        let socket = Socket::new(SocketDomain::Inet, SocketType::Stream, 0)?;
        Ok(HttpClient { socket })
    }

    /// Performs an HTTP GET request to the specified URL
    /// Returns the response body as bytes
    pub fn get(url: &str) -> Result<alloc::vec::Vec<u8>, Error> {
        // Parse URL (simplified - assumes http:// format)
        if !url.starts_with("http://") {
            return Err(Error::from_i64(-22)); // EINVAL
        }

        let url_without_scheme = &url[7..]; // Remove "http://"
        let parts: alloc::vec::Vec<&str> = url_without_scheme.split('/').collect();
        if parts.is_empty() {
            return Err(Error::from_i64(-22));
        }

        let host = parts[0];
        let path = if parts.len() > 1 {
            let mut path_str = alloc::string::String::from("/");
            for i in 1..parts.len() {
                path_str.push_str(parts[i]);
                if i < parts.len() - 1 {
                    path_str.push('/');
                }
            }
            path_str
        } else {
            alloc::string::String::from("/")
        };

        // Resolve hostname
        let mut ip = [0u8; 4];
        resolve(host, &mut ip)?;

        // Connect to server (port 80 for HTTP)
        let addr = SockAddrIn::new(ip, 80);
        let client = Self::new()?;
        client.socket.connect(addr.as_bytes())?;

        // Send HTTP GET request
        let request = alloc::format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            path,
            host
        );
        client.socket.write(request.as_bytes())?;

        // Read response
        // ponytail: 32MB cap bounds memory against a hostile/hung server; raise if packages outgrow it
        const MAX_RESPONSE: usize = 32 * 1024 * 1024;
        let mut response = alloc::vec::Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            match client.socket.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    if response.len() + n > MAX_RESPONSE {
                        return Err(Error::from_i64(-28)); // ENOSPC
                    }
                    response.extend_from_slice(&buffer[..n]);
                }
                Err(_) => break,
            }
        }

        // Skip HTTP headers (find double CRLF)
        if let Some(header_end) = find_double_crlf(&response) {
            let body = response[header_end + 4..].to_vec();
            Ok(body)
        } else {
            Ok(response)
        }
    }
}

/// Find the position of \r\n\r\n in a byte slice
fn find_double_crlf(data: &[u8]) -> Option<usize> {
    (0..data.len().saturating_sub(3)).find(|&i| {
        data[i] == b'\r' && data[i + 1] == b'\n' && data[i + 2] == b'\r' && data[i + 3] == b'\n'
    })
}
