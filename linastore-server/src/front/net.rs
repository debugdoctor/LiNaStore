use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Duration;

use socket2::{SockRef, TcpKeepalive};
use tokio::net::{TcpListener, TcpSocket, TcpStream};

/// Listen backlog shared by all frontends. Tokio's implicit default is 1024;
/// a larger value absorbs connection bursts until `accept()` drains them.
const LISTEN_BACKLOG: u32 = 4096;

/// Upper bound on how long a peer may hold a connection while not sending a
/// complete request header. Sheds slowloris-style holds and idle keep-alive
/// connections so fds/tasks stay proportional to active traffic.
pub const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(60);

/// Bind a listener with an explicit backlog and address reuse. Accepts the same
/// `ip:port` (or hostname) shape as `TcpListener::bind`.
pub fn bind_listener(addr: &str) -> io::Result<TcpListener> {
    let sockaddr: SocketAddr = addr
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("no address in {addr}")))?;

    let socket = match sockaddr {
        SocketAddr::V4(_) => TcpSocket::new_v4()?,
        SocketAddr::V6(_) => TcpSocket::new_v6()?,
    };

    socket.set_reuseaddr(true)?;
    socket.bind(sockaddr)?;
    socket.listen(LISTEN_BACKLOG)
}

/// TCP keepalive idle time before the first probe is sent.
const KEEPALIVE_IDLE: Duration = Duration::from_secs(60);
/// Interval between keepalive probes.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(20);
/// Failed probes before the kernel closes the connection.
#[cfg(not(any(target_os = "openbsd", target_os = "redox", target_os = "windows")))]
const KEEPALIVE_RETRIES: u32 = 3;

/// Disable Nagle so small responses are not delayed by packet coalescing; this
/// is request/response traffic where latency matters more than segment count.
///
/// Also enable TCP keepalive with a short idle window. This reclaims
/// connections whose peer vanished without a FIN/RST (unplugged cable, expired
/// NAT entry, crashed host) — resource hygiene, not flood protection. The OS
/// default of 2h is far too slow to be useful here.
pub fn tune_stream(stream: &TcpStream) {
    let _ = stream.set_nodelay(true);

    let keepalive = TcpKeepalive::new()
        .with_time(KEEPALIVE_IDLE)
        .with_interval(KEEPALIVE_INTERVAL);
    #[cfg(not(any(target_os = "openbsd", target_os = "redox", target_os = "windows")))]
    let keepalive = keepalive.with_retries(KEEPALIVE_RETRIES);

    let _ = SockRef::from(stream).set_tcp_keepalive(&keepalive);
}
