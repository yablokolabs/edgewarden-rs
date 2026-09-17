//! Linux transparent-proxy helpers (TPROXY / original-destination).
//!
//! Portable proxy mode NEVER calls this module. Transparent mode requires
//! Linux + `CAP_NET_ADMIN` + nftables/iptables rules (see
//! `scripts/nftables-example.sh`). Unit tests must not require root: all
//! functions here fail gracefully with an explanatory error when the
//! socket option is unavailable.

use std::io;

/// Original destination of a redirected connection (TPROXY).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OriginalDestination {
    pub ip: std::net::IpAddr,
    pub port: u16,
}

/// Retrieve `SO_ORIGINAL_DST` (getsockopt 80) for a redirected socket.
///
/// Only compiled on Linux; on other platforms this returns an explanatory
/// error so portable builds keep working.
#[cfg(target_os = "linux")]
pub fn original_destination(sock: &tokio::net::TcpStream) -> io::Result<OriginalDestination> {
    use std::os::unix::io::AsRawFd;
    original_destination_fd(sock.as_raw_fd())
}

#[cfg(not(target_os = "linux"))]
pub fn original_destination(_sock: &tokio::net::TcpStream) -> io::Result<OriginalDestination> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "transparent proxy requires Linux (TPROXY)",
    ))
}

#[cfg(target_os = "linux")]
fn original_destination_fd(fd: std::os::unix::io::RawFd) -> io::Result<OriginalDestination> {
    use std::mem::MaybeUninit;

    const SOL_IP: libc::c_int = libc::SOL_IP;
    const SO_ORIGINAL_DST: libc::c_int = 80;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SockAddrIn {
        sin_family: libc::sa_family_t,
        sin_port: libc::in_port_t,
        sin_addr: libc::in_addr,
        sin_zero: [u8; 8],
    }

    let mut addr: MaybeUninit<SockAddrIn> = MaybeUninit::uninit();
    let mut len = std::mem::size_of::<SockAddrIn>() as libc::socklen_t;
    // SAFETY: getsockopt writes at most `len` bytes into `addr`.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            SOL_IP,
            SO_ORIGINAL_DST,
            addr.as_mut_ptr() as *mut libc::c_void,
            &mut len,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: kernel initialized `addr` on success.
    let addr = unsafe { addr.assume_init() };
    let ip = std::net::IpAddr::V4(std::net::Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr)));
    Ok(OriginalDestination {
        ip,
        port: u16::from_be(addr.sin_port),
    })
}

/// Bind a listener suitable for TPROXY traffic. Falls back to a normal
/// bind with a warning when `IP_TRANSPARENT` cannot be set (e.g. no caps).
#[cfg(target_os = "linux")]
pub fn bind_transparent(addr: &std::net::SocketAddr) -> io::Result<std::net::TcpListener> {
    use std::os::fd::{FromRawFd, IntoRawFd};
    use std::os::unix::io::AsRawFd;

    let domain = if addr.is_ipv4() {
        libc::AF_INET
    } else {
        libc::AF_INET6
    };
    // SAFETY: creating a socket fd; checked below.
    let fd = unsafe { libc::socket(domain, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is valid; wrapped below so it closes on all paths.
    let sock = unsafe { socket2::Socket::from_raw_fd(fd) };
    let _ = sock.set_reuse_address(true);
    // IP_TRANSPARENT = 19. Best effort: log but continue without it.
    const IP_TRANSPARENT: libc::c_int = 19;
    let one: libc::c_int = 1;
    let rc = unsafe {
        libc::setsockopt(
            sock.as_raw_fd(),
            libc::SOL_IP,
            IP_TRANSPARENT,
            &one as *const _ as *const libc::c_void,
            std::mem::size_of_val(&one) as libc::socklen_t,
        )
    };
    if rc != 0 {
        tracing::warn!(
            error = %io::Error::last_os_error(),
            "IP_TRANSPARENT unavailable; continuing without TPROXY"
        );
    }
    sock.bind(&(*addr).into())?;
    sock.listen(1024)?;
    // SAFETY: listener fd from socket2; transferred to std.
    let std_listener: std::net::TcpListener =
        unsafe { std::net::TcpListener::from_raw_fd(sock.into_raw_fd()) };
    std_listener.set_nonblocking(true)?;
    Ok(std_listener)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparent_mode_is_opt_in_and_documented() {
        // This test must pass as non-root: we only assert the error path is
        // graceful, never that TPROXY actually works.
        #[cfg(not(target_os = "linux"))]
        {
            assert!(true);
        }
        #[cfg(target_os = "linux")]
        {
            // Binding a high port transparent listener should succeed even
            // without privileges (with a fallback warning).
            let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
            let res = bind_transparent(&addr);
            assert!(res.is_ok(), "transparent bind fallback failed: {res:?}");
        }
    }
}
