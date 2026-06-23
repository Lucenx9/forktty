//! Real PTY/process persistence for generic terminal panes.
//!
//! ForkTTY's embedded Ghostty surfaces own their child PTY for the lifetime of
//! the GTK process: the embedding ABI (see `ghostty_gtk_embed.rs`) exposes no
//! way to detach a running PTY from one surface and re-attach it to a freshly
//! spawned surface after a UI restart. So when the GTK process exits, every
//! embedded surface's child PTY is torn down with it. Session restore only
//! re-spawns fresh shells and replays saved scrollback; the *processes*
//! (shells, dev servers, REPLs, editors, long-running commands) do not survive.
//!
//! To make a generic terminal's process tree actually survive a GTK restart,
//! ForkTTY can run the workload under a detach/reattach session broker
//! (`dtach`). The broker keeps the real program under its own PTY inside a
//! detached daemon; the embedded Ghostty surface only runs the broker *client*,
//! which dies with the GTK process. On relaunch ForkTTY spawns a fresh client
//! that re-attaches to the surviving daemon, keyed by a per-surface socket path
//! derived from the *persisted* surface id — so no extra session state is
//! required, the existing surface id is the durable handle.
//!
//! This module is pure: it detects an available broker and builds the wrapped
//! argv and socket path. It never spawns processes or performs the attach
//! itself; that stays in the GTK terminal-spawn path so all real work remains
//! visible in a terminal pane.

use crate::command_safety::{is_executable_file, is_shell_trampoline};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[cfg(unix)]
use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};

/// Directory (under the per-user runtime dir) that holds broker sockets.
pub const PTY_SESSION_DIR_NAME: &str = "forktty-pty";

/// Longest surface id accepted as a socket-filename component. Surface ids are
/// short (`surface-1234`); a generous cap keeps the socket path well under the
/// `sun_path` limit while rejecting absurd input.
const MAX_SURFACE_ID_LEN: usize = 64;

/// Linux `sockaddr_un.sun_path` is 108 bytes including the trailing NUL. Keep a
/// little headroom so the full broker socket path is accepted by dtach and the
/// kernel instead of failing only after the terminal spawn starts.
const MAX_SOCKET_PATH_BYTES: usize = 100;

/// Detach/reattach brokers ForkTTY knows how to drive. Only `dtach` is wired up
/// today; it takes an explicit socket path, which matches ForkTTY's owner-only
/// runtime-dir security model exactly. The enum leaves room for `abduco`/others
/// later without changing call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyBroker {
    Dtach,
}

impl PtyBroker {
    /// The program name looked up on `PATH`.
    pub fn program_name(self) -> &'static str {
        match self {
            PtyBroker::Dtach => "dtach",
        }
    }
}

/// A resolved, available persistence broker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyPersistence {
    pub broker: PtyBroker,
    /// Absolute, executable path to the broker program.
    pub broker_path: PathBuf,
}

/// Concrete plan to run one surface's command under a broker: which broker,
/// where its socket lives, and (implicitly, via the socket path) which surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyPersistencePlan {
    broker: PtyBroker,
    broker_path: String,
    socket_path: PathBuf,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PtyPersistenceError {
    #[error("surface id is not a safe persistence socket filename: {0}")]
    UnsafeSurfaceId(String),
    #[error("persistence socket path is too long: {actual} bytes > {max} bytes")]
    SocketPathTooLong { actual: usize, max: usize },
    #[error("persistence broker path is not valid UTF-8")]
    NonUtf8BrokerPath,
    #[error("persistence socket path is not valid UTF-8")]
    NonUtf8SocketPath,
    #[error("cannot persist an empty command")]
    EmptyCommand,
    #[error("refusing to persist a shell trampoline command")]
    ShellTrampoline,
}

/// Detect an available broker using the process `PATH`. Returns `None` when no
/// known broker is installed; callers then fall back to an ephemeral spawn.
pub fn detect() -> Option<PtyPersistence> {
    detect_with_path(std::env::var_os("PATH").as_deref())
}

/// `detect`, but with an explicit `PATH` value so the logic is testable.
///
/// Only absolute `PATH` entries are consulted, mirroring
/// `forktty_terminal::spawn::resolve_child_program`: the broker is spawned as a
/// child of an embedded surface whose cwd is attacker-influenced, so a relative
/// or empty `PATH` component must never resolve the broker.
pub fn detect_with_path(path: Option<&OsStr>) -> Option<PtyPersistence> {
    for broker in [PtyBroker::Dtach] {
        if let Some(broker_path) = resolve_on_absolute_path(broker.program_name(), path) {
            return Some(PtyPersistence {
                broker,
                broker_path,
            });
        }
    }
    None
}

fn resolve_on_absolute_path(program: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    std::env::split_paths(path?)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable_file(candidate))
}

/// Whether `surface_id` is safe to use as a socket-filename component: a short,
/// non-empty string of `[A-Za-z0-9._-]` with no `.`/`..` traversal. ForkTTY's
/// own surface ids (`surface-N`) always pass; rejecting anything else keeps a
/// crafted persisted session from steering the socket outside its directory.
fn is_safe_surface_id(surface_id: &str) -> bool {
    if surface_id.is_empty() || surface_id.len() > MAX_SURFACE_ID_LEN {
        return false;
    }
    if surface_id == "." || surface_id == ".." {
        return false;
    }
    surface_id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

#[cfg(unix)]
fn socket_path_len(path: &Path) -> usize {
    path.as_os_str().as_bytes().len()
}

#[cfg(not(unix))]
fn socket_path_len(path: &Path) -> usize {
    path.to_string_lossy().len()
}

/// Per-surface broker socket path: `<runtime_dir>/forktty-pty/<surface_id>.sock`.
/// Pure: it validates and joins; it does not touch the filesystem.
pub fn session_socket_path(
    runtime_dir: &Path,
    surface_id: &str,
) -> Result<PathBuf, PtyPersistenceError> {
    if !is_safe_surface_id(surface_id) {
        return Err(PtyPersistenceError::UnsafeSurfaceId(surface_id.to_string()));
    }
    let socket = runtime_dir
        .join(PTY_SESSION_DIR_NAME)
        .join(format!("{surface_id}.sock"));
    let len = socket_path_len(&socket);
    if len > MAX_SOCKET_PATH_BYTES {
        return Err(PtyPersistenceError::SocketPathTooLong {
            actual: len,
            max: MAX_SOCKET_PATH_BYTES,
        });
    }
    Ok(socket)
}

/// Create the socket's parent directory with owner-only (`0700`) permissions,
/// matching the runtime-dir owner-only guarantee the socket layer relies on.
/// Idempotent. Callers invoke this just before handing the socket path to the
/// broker.
pub fn ensure_private_session_dir(socket_path: &Path) -> std::io::Result<()> {
    let Some(parent) = socket_path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent)?;
    #[cfg(unix)]
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

/// Best-effort cleanup when a ForkTTY surface is explicitly closed or
/// restarted. UI process shutdown must not call this path: the broker socket is
/// the persisted handle used to reattach after relaunch. Explicit pane removal
/// is different; unlinking the socket prevents a future reused surface id from
/// accidentally reattaching to a stale detached session.
pub fn cleanup_session_socket(runtime_dir: &Path, surface_id: &str) -> std::io::Result<bool> {
    let socket = session_socket_path(runtime_dir, surface_id)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err.to_string()))?;
    match std::fs::remove_file(&socket) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

impl PtyPersistencePlan {
    pub fn new(
        persistence: &PtyPersistence,
        socket_path: PathBuf,
    ) -> Result<Self, PtyPersistenceError> {
        let broker_path = persistence
            .broker_path
            .to_str()
            .ok_or(PtyPersistenceError::NonUtf8BrokerPath)?
            .to_string();
        Ok(Self {
            broker: persistence.broker,
            broker_path,
            socket_path,
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Wrap a resolved command argv so it runs under the broker in
    /// attach-or-create mode. `command[0]` must already be an absolute,
    /// resolved program path (the embedded-spawn path resolves it before
    /// wrapping). Refuses an empty command or a `sh -c` trampoline so the
    /// no-`sh -c` argv policy holds across the broker boundary too.
    ///
    /// dtach form: `dtach -A <socket> -E -z <program> <args...>` — the socket
    /// follows `-A`, `-E` disables the detach key so it never swallows the
    /// user's keystrokes, and `-z` passes the suspend key through to the child.
    /// The command is used only when the session is first created; a later
    /// attach reuses the already-running process tree.
    pub fn wrap_command(&self, command: Vec<String>) -> Result<Vec<String>, PtyPersistenceError> {
        if command.is_empty() {
            return Err(PtyPersistenceError::EmptyCommand);
        }
        if is_shell_trampoline(&command[0], &command[1..]) {
            return Err(PtyPersistenceError::ShellTrampoline);
        }
        let socket = self
            .socket_path
            .to_str()
            .ok_or(PtyPersistenceError::NonUtf8SocketPath)?;
        match self.broker {
            PtyBroker::Dtach => {
                let mut argv = Vec::with_capacity(command.len() + 5);
                argv.push(self.broker_path.clone());
                argv.push("-A".to_string());
                argv.push(socket.to_string());
                argv.push("-E".to_string());
                argv.push("-z".to_string());
                argv.extend(command);
                Ok(argv)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn dtach_persistence() -> PtyPersistence {
        PtyPersistence {
            broker: PtyBroker::Dtach,
            broker_path: PathBuf::from("/usr/bin/dtach"),
        }
    }

    fn plan(socket: &str) -> PtyPersistencePlan {
        PtyPersistencePlan::new(&dtach_persistence(), PathBuf::from(socket)).unwrap()
    }

    #[test]
    fn wrap_command_builds_attach_or_create_dtach_argv() {
        let argv = plan("/run/user/1000/forktty-pty/surface-7.sock")
            .wrap_command(vec![
                "/usr/bin/bash".to_string(),
                "-l".to_string(),
                "-i".to_string(),
            ])
            .unwrap();

        assert_eq!(
            argv,
            vec![
                "/usr/bin/dtach",
                "-A",
                "/run/user/1000/forktty-pty/surface-7.sock",
                "-E",
                "-z",
                "/usr/bin/bash",
                "-l",
                "-i",
            ]
        );
    }

    #[test]
    fn wrap_command_keeps_socket_immediately_after_attach_flag() {
        // dtach parses the socket as the first positional after `-A`; the
        // behavior flags and command must follow it, never precede it.
        let argv = plan("/run/sock")
            .wrap_command(vec!["/bin/zsh".to_string()])
            .unwrap();
        assert_eq!(&argv[0..3], &["/usr/bin/dtach", "-A", "/run/sock"]);
        assert_eq!(argv.last().unwrap(), "/bin/zsh");
    }

    #[test]
    fn wrap_command_rejects_empty_command() {
        assert_eq!(
            plan("/run/sock").wrap_command(Vec::new()),
            Err(PtyPersistenceError::EmptyCommand)
        );
    }

    #[test]
    fn wrap_command_refuses_shell_trampoline() {
        // Persisting `sh -c "..."` would both be pointless and smuggle a shell
        // command string across the broker boundary; the no-`sh -c` policy
        // rejects it.
        assert_eq!(
            plan("/run/sock").wrap_command(vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "rm -rf ~".to_string(),
            ]),
            Err(PtyPersistenceError::ShellTrampoline)
        );
    }

    #[test]
    fn wrap_command_allows_interactive_login_shell() {
        // An interactive login shell is not a trampoline and is exactly the
        // generic workload we want to persist.
        assert!(plan("/run/sock")
            .wrap_command(vec![
                "/bin/bash".to_string(),
                "-l".to_string(),
                "-i".to_string()
            ])
            .is_ok());
    }

    #[test]
    fn session_socket_path_is_namespaced_under_runtime_dir() {
        let socket = session_socket_path(Path::new("/run/user/1000"), "surface-12").unwrap();
        assert_eq!(
            socket,
            PathBuf::from("/run/user/1000/forktty-pty/surface-12.sock")
        );
    }

    #[test]
    fn session_socket_path_rejects_traversal_and_separators() {
        for bad in ["../escape", "a/b", "..", ".", "", "with space", "ctrl\u{3}"] {
            assert!(
                session_socket_path(Path::new("/run/user/1000"), bad).is_err(),
                "expected rejection for {bad:?}"
            );
        }
    }

    #[test]
    fn session_socket_path_rejects_overlong_id() {
        let long = "a".repeat(MAX_SURFACE_ID_LEN + 1);
        assert!(session_socket_path(Path::new("/run/user/1000"), &long).is_err());
    }

    #[test]
    fn session_socket_path_rejects_overlong_full_path() {
        let long_runtime_dir = PathBuf::from(format!("/{}", "x".repeat(90)));
        assert!(matches!(
            session_socket_path(&long_runtime_dir, "surface-1"),
            Err(PtyPersistenceError::SocketPathTooLong { .. })
        ));
    }

    #[test]
    fn safe_surface_id_accepts_forktty_ids() {
        assert!(is_safe_surface_id("surface-20"));
        assert!(is_safe_surface_id("surface_20"));
        assert!(is_safe_surface_id("s20"));
    }

    #[cfg(unix)]
    #[test]
    fn detect_with_path_resolves_dtach_on_absolute_entries_only() {
        let dir = tempfile::tempdir().unwrap();
        let abs = dir.path().join("abs");
        fs::create_dir(&abs).unwrap();
        let dtach = abs.join("dtach");
        fs::write(&dtach, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&dtach, fs::Permissions::from_mode(0o755)).unwrap();

        // A relative PATH entry that also contains a `dtach` must be ignored.
        let path = format!(".:{}", abs.display());
        let detected = detect_with_path(Some(OsStr::new(&path))).expect("dtach detected");
        assert_eq!(detected.broker, PtyBroker::Dtach);
        assert_eq!(detected.broker_path, dtach.canonicalize().unwrap_or(dtach));
    }

    #[test]
    fn detect_with_path_returns_none_when_no_broker_present() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            detect_with_path(Some(OsStr::new(dir.path().to_str().unwrap()))),
            None
        );
        assert_eq!(detect_with_path(None), None);
    }

    #[cfg(unix)]
    #[test]
    fn ensure_private_session_dir_creates_owner_only_directory() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join(PTY_SESSION_DIR_NAME).join("surface-1.sock");

        ensure_private_session_dir(&socket).unwrap();

        let parent = socket.parent().unwrap();
        assert!(parent.is_dir());
        assert_eq!(
            fs::metadata(parent).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn cleanup_session_socket_removes_existing_socket() {
        let dir = tempfile::tempdir().unwrap();
        let socket = session_socket_path(dir.path(), "surface-9").unwrap();
        ensure_private_session_dir(&socket).unwrap();
        fs::write(&socket, b"placeholder").unwrap();

        assert!(cleanup_session_socket(dir.path(), "surface-9").unwrap());
        assert!(!socket.exists());
        assert!(!cleanup_session_socket(dir.path(), "surface-9").unwrap());
    }

    #[test]
    fn cleanup_session_socket_rejects_unsafe_surface_id() {
        let dir = tempfile::tempdir().unwrap();
        let err = cleanup_session_socket(dir.path(), "../surface-9").unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }
}
