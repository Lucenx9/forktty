use crate::env_var;
use serde_json::Value;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener as StdUnixListener, UnixStream as StdUnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const SOCKET_PROBE_TIMEOUT: Duration = Duration::from_millis(250);

/// Distinguishes concurrent [`bind_private_socket_path`] staging directories
/// within one process (tests bind many listeners in parallel).
static SOCKET_BIND_STAGING_SEQ: AtomicU64 = AtomicU64::new(0);

pub fn default_socket_path() -> PathBuf {
    default_socket_dir().join("forktty.sock")
}

/// Resolve the socket path from an optional override, falling back to the
/// default. An override is honored only when it trims to a non-empty absolute
/// path; anything else (relative, blank, unset) uses [`default_socket_path`].
pub fn socket_path_from_env(socket_env: Option<String>) -> PathBuf {
    socket_env
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && Path::new(value).is_absolute())
        .map(PathBuf::from)
        .unwrap_or_else(default_socket_path)
}

pub fn bind_socket_listener(
    socket_path: impl AsRef<Path>,
    enforce_private_parent: bool,
) -> io::Result<StdUnixListener> {
    let socket_path = socket_path.as_ref();
    prepare_socket_parent(socket_path, enforce_private_parent)?;
    // Removing a stale socket and re-binding is not atomic: a racing ForkTTY
    // start can recreate the path between `remove_file` and `bind`, surfacing a
    // bare `AddrInUse`. Re-inspect once on that error so the occupant is
    // reported accurately instead of as a confusing bind failure.
    let mut reclaimed_stale = false;
    let listener = loop {
        match fs::symlink_metadata(socket_path) {
            Ok(metadata) => {
                if !metadata.file_type().is_socket() {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        format!(
                            "refusing to replace non-socket path at {}",
                            socket_path.display()
                        ),
                    ));
                }
                match inspect_existing_socket(socket_path) {
                    ExistingSocketOccupant::ForkTTY => {
                        return Err(io::Error::new(
                            io::ErrorKind::AddrInUse,
                            format!(
                                "another ForkTTY instance is already using {}",
                                socket_path.display()
                            ),
                        ));
                    }
                    ExistingSocketOccupant::Other => {
                        return Err(io::Error::new(
                            io::ErrorKind::AddrInUse,
                            format!("socket path {} is already in use", socket_path.display()),
                        ));
                    }
                    ExistingSocketOccupant::Stale => fs::remove_file(socket_path)?,
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        match bind_private_socket_path(socket_path) {
            Ok(listener) => break listener,
            Err(err) if err.kind() == io::ErrorKind::AddrInUse && !reclaimed_stale => {
                reclaimed_stale = true;
                continue;
            }
            Err(err) => return Err(err),
        }
    };
    listener.set_nonblocking(true)?;
    if let Err(err) = fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600)) {
        let _ = fs::remove_file(socket_path);
        return Err(err);
    }
    Ok(listener)
}

/// Binds the socket without it ever being group/other-accessible at its public
/// path: the listener is created inside a chmod-0700 staging directory (whose
/// search bit shields the inode regardless of umask), chmod-0600, and only then
/// hard-linked into place. Linking fails with `AddrInUse` if the public path
/// appeared in the meantime, preserving the caller's stale-reclaim retry.
///
/// Deliberately NOT implemented with a process-wide `umask()` flip: the umask
/// is global, so flipping it poisons the creation mode of every file or
/// directory any other thread makes during the window (this broke concurrent
/// tests' `tempdir()` with mode-0600 directories).
fn bind_private_socket_path(socket_path: &Path) -> io::Result<StdUnixListener> {
    let parent = socket_path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket path has no parent directory",
        )
    })?;
    let staging = parent.join(format!(
        ".forktty-bind-{}-{}",
        std::process::id(),
        SOCKET_BIND_STAGING_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    // A leftover directory can only come from a crashed process that had this
    // pid in a previous boot; the sequence number never repeats within one.
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir(&staging)?;
    let result = bind_in_staging_dir(&staging, socket_path);
    let _ = fs::remove_dir_all(&staging);
    result
}

fn bind_in_staging_dir(staging: &Path, socket_path: &Path) -> io::Result<StdUnixListener> {
    fs::set_permissions(staging, fs::Permissions::from_mode(0o700))?;
    let staged = staging.join("sock");
    let listener = StdUnixListener::bind(&staged)?;
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o600))?;
    fs::hard_link(&staged, socket_path).map_err(|err| {
        if err.kind() == io::ErrorKind::AlreadyExists {
            io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("socket path {} is already in use", socket_path.display()),
            )
        } else {
            err
        }
    })?;
    Ok(listener)
}

enum ExistingSocketOccupant {
    Stale,
    ForkTTY,
    Other,
}

fn inspect_existing_socket(path: &Path) -> ExistingSocketOccupant {
    match StdUnixStream::connect(path) {
        Ok(stream) => match probe_forktty_socket(stream) {
            Ok(true) => ExistingSocketOccupant::ForkTTY,
            Ok(false) | Err(_) => ExistingSocketOccupant::Other,
        },
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            ExistingSocketOccupant::Stale
        }
        Err(_) => ExistingSocketOccupant::Other,
    }
}

// Cap how much the probe will buffer from a foreign socket while waiting for
// a newline. A genuine ForkTTY pong response is ~50 bytes; anything dramatically
// larger almost certainly comes from an unrelated peer that bound to our path,
// and we don't want to grow the response buffer indefinitely while the timeout
// drains.
#[cfg(test)]
pub(crate) const PROBE_RESPONSE_MAX_BYTES: usize = 4096;
#[cfg(not(test))]
const PROBE_RESPONSE_MAX_BYTES: usize = 4096;

fn probe_forktty_socket(stream: StdUnixStream) -> io::Result<bool> {
    probe_forktty_socket_with_timeout(stream, SOCKET_PROBE_TIMEOUT)
}

// The timeout is injectable so tests of the protocol/parsing behavior can pass
// a generous one: the responding peer is a freshly spawned thread, and under
// scheduler starvation the production 250ms can elapse before it ever runs.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn probe_forktty_socket_with_timeout(
    mut stream: StdUnixStream,
    timeout: Duration,
) -> io::Result<bool> {
    use std::io::{Read, Write};
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.write_all(br#"{"id":"probe","method":"system.ping","params":{}}"#)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut response = Vec::with_capacity(256);
    let mut buf = [0u8; 256];
    // Overall deadline on top of the per-read timeouts: a peer that trickles
    // one byte per read window never trips the read timeout and would stretch
    // the probe (which runs on the GTK main thread at startup) almost
    // indefinitely. A real ForkTTY answers a ping in one write, so anything
    // still dribbling after a few timeout periods is a foreign peer.
    let started = std::time::Instant::now();
    let overall_deadline = timeout * 4;
    loop {
        if started.elapsed() > overall_deadline {
            return Ok(false);
        }
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let chunk = &buf[..n];
        if let Some(pos) = chunk.iter().position(|&b| b == b'\n') {
            response.extend_from_slice(&chunk[..pos]);
            break;
        }
        if response.len().saturating_add(n) > PROBE_RESPONSE_MAX_BYTES {
            return Ok(false);
        }
        response.extend_from_slice(chunk);
    }
    if response.is_empty() {
        return Ok(false);
    }
    let value: Value = serde_json::from_slice(&response)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    Ok(value.get("id").and_then(Value::as_str) == Some("probe")
        && value.get("ok").and_then(Value::as_bool) == Some(true)
        && value.get("result").and_then(Value::as_str) == Some("pong"))
}

fn prepare_socket_parent(socket_path: &Path, enforce_private_parent: bool) -> io::Result<()> {
    let parent = socket_path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("socket path has no parent: {}", socket_path.display()),
        )
    })?;
    if !parent.exists() {
        let mut builder = DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(parent)?;
    }
    if enforce_private_parent {
        validate_private_socket_parent(parent)?;
    }
    Ok(())
}

fn validate_private_socket_parent(path: &Path) -> io::Result<()> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("socket parent is not a directory: {}", path.display()),
        ));
    }
    if metadata.uid() != effective_uid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "socket parent {} is owned by uid {}, expected {}",
                path.display(),
                metadata.uid(),
                effective_uid()
            ),
        ));
    }
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "socket parent {} must not be accessible by group/other (mode {:o})",
                path.display(),
                mode
            ),
        ));
    }
    Ok(())
}

fn default_socket_dir() -> PathBuf {
    default_socket_dir_from_env(env_var("XDG_RUNTIME_DIR").ok().as_deref())
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn default_socket_dir_from_env(runtime_dir: Option<&str>) -> PathBuf {
    if let Some(runtime_dir) = runtime_dir.map(str::trim).filter(|value| !value.is_empty()) {
        let path = PathBuf::from(runtime_dir);
        if path.is_absolute() {
            return path;
        }
    }
    std::env::temp_dir().join(format!("forktty-{}", effective_uid()))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn effective_uid() -> u32 {
    unsafe { libc::geteuid() as u32 }
}
