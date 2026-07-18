//! Bounded blocking `AF_UNIX` client connections shared by socket probes and clients.
//!
//! Linux reports `EAGAIN` when an `AF_UNIX` listener backlog is full. That
//! attempt is not pending, so retries must use a fresh descriptor and remain
//! bounded by one overall deadline. Successful streams are restored to blocking
//! mode before they cross this module boundary.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

const BACKLOG_RETRY_INTERVAL: Duration = Duration::from_millis(20);

/// Connect to a filesystem Unix socket within `timeout`.
///
/// The returned stream is in blocking mode. A full accept backlog yields
/// [`io::ErrorKind::TimedOut`] when the overall deadline expires.
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidInput`] when the path is empty, contains a
/// NUL byte, does not fit in `sockaddr_un::sun_path`, or when `timeout` cannot
/// be represented as an [`Instant`] deadline. Returns
/// [`io::ErrorKind::TimedOut`] if the deadline expires, including while retrying
/// a full listener backlog. Other errors report failures from socket creation,
/// `connect(2)`, `poll(2)`, `SO_ERROR`, or restoring blocking mode.
pub fn connect_unix_stream_with_timeout(
    socket_path: &Path,
    timeout: Duration,
) -> io::Result<UnixStream> {
    let (addr, addr_len) = unix_socket_address(socket_path)?;
    let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket connect timeout is too large",
        )
    })?;

    loop {
        ensure_before_deadline(deadline)?;
        let fd = create_nonblocking_socket()?;
        match connect_once(fd.as_raw_fd(), &addr, addr_len, deadline)? {
            ConnectOutcome::Connected => {
                set_blocking(fd.as_raw_fd())?;
                return Ok(UnixStream::from(fd));
            }
            ConnectOutcome::BacklogFull => {
                // Dropping this descriptor is required: AF_UNIX EAGAIN does not
                // leave a connect in progress, and retrying connect(2) on the
                // same descriptor has kernel-dependent failure behavior.
                drop(fd);
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(backlog_timeout());
                }
                std::thread::sleep(remaining.min(BACKLOG_RETRY_INTERVAL));
                if Instant::now() >= deadline {
                    return Err(backlog_timeout());
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectOutcome {
    Connected,
    BacklogFull,
}

fn create_nonblocking_socket() -> io::Result<OwnedFd> {
    // SAFETY: plain socket(2) call; the result is checked before ownership is
    // transferred to `OwnedFd`.
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a fresh descriptor owned by this function.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn connect_once(
    fd: RawFd,
    addr: &libc::sockaddr_un,
    addr_len: libc::socklen_t,
    deadline: Instant,
) -> io::Result<ConnectOutcome> {
    loop {
        ensure_before_deadline(deadline)?;
        // SAFETY: `addr` is a valid sockaddr_un of `addr_len` bytes and `fd` is
        // a live socket owned by the caller.
        let rc = unsafe {
            libc::connect(
                fd,
                addr as *const libc::sockaddr_un as *const libc::sockaddr,
                addr_len,
            )
        };
        if rc == 0 {
            ensure_before_deadline(deadline)?;
            return Ok(ConnectOutcome::Connected);
        }

        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(libc::EISCONN) => {
                ensure_before_deadline(deadline)?;
                return Ok(ConnectOutcome::Connected);
            }
            Some(libc::EINPROGRESS) => {
                poll_writable_until(fd, deadline)?;
                return match take_socket_error(fd)? {
                    0 => {
                        ensure_before_deadline(deadline)?;
                        Ok(ConnectOutcome::Connected)
                    }
                    libc::EAGAIN => Ok(ConnectOutcome::BacklogFull),
                    code => Err(io::Error::from_raw_os_error(code)),
                };
            }
            Some(libc::EAGAIN) => return Ok(ConnectOutcome::BacklogFull),
            _ => return Err(error),
        }
    }
}

pub(crate) fn unix_socket_address(path: &Path) -> io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: all-zero is a valid bit pattern for sockaddr_un.
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.is_empty() || bytes.contains(&0) || bytes.len() >= addr.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket path is empty, contains NUL, or is too long for sun_path",
        ));
    }
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (dst, src) in addr.sun_path.iter_mut().zip(bytes) {
        *dst = *src as libc::c_char;
    }
    let len = std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1;
    Ok((addr, len as libc::socklen_t))
}

fn poll_writable_until(fd: RawFd, deadline: Instant) -> io::Result<()> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(connect_timeout());
        }
        let mut poll_fd = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        let millis = remaining.as_millis().clamp(1, i32::MAX as u128) as libc::c_int;
        // SAFETY: `poll_fd` is valid for the duration of poll(2).
        let rc = unsafe { libc::poll(&mut poll_fd, 1, millis) };
        if rc > 0 {
            ensure_before_deadline(deadline)?;
            return Ok(());
        }
        if rc == 0 {
            return Err(connect_timeout());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn ensure_before_deadline(deadline: Instant) -> io::Result<()> {
    if Instant::now() >= deadline {
        return Err(connect_timeout());
    }
    Ok(())
}

fn take_socket_error(fd: RawFd) -> io::Result<libc::c_int> {
    let mut socket_error: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: `socket_error` and `len` are valid out-pointers for SO_ERROR.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_ERROR,
            &mut socket_error as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(socket_error)
}

fn set_blocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: fcntl(2) operates on a live descriptor owned by the caller.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: this updates only the descriptor's file status flags.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn connect_timeout() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        "timed out connecting to the socket",
    )
}

fn backlog_timeout() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        "timed out waiting for the socket accept backlog to drain",
    )
}
