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
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::time::Duration;
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

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PtyPersistenceCleanup {
    pub sockets_removed: usize,
    pub processes_signaled: usize,
    pub process_signal_errors: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagedDtachSocket {
    socket_path: PathBuf,
    surface_id: String,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagedDtachProcess {
    pid: libc::pid_t,
    socket: ManagedDtachSocket,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct LinuxProcessInfo {
    pid: libc::pid_t,
    ppid: libc::pid_t,
    args: Vec<OsString>,
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

/// Best-effort cleanup for managed persistence sessions. `preserve_surface_ids`
/// lets the GTK runtime avoid killing currently visible panes when the setting
/// is disabled live; startup can pass an empty set to discard all old sessions
/// before any reattach is possible.
pub fn cleanup_managed_sessions(
    runtime_dir: &Path,
    preserve_surface_ids: &BTreeSet<String>,
) -> io::Result<PtyPersistenceCleanup> {
    let mut summary = PtyPersistenceCleanup::default();

    #[cfg(target_os = "linux")]
    {
        let process_infos = linux_process_infos()?;
        let managed = managed_dtach_processes_from_infos(&process_infos, runtime_dir);
        let targets = termination_targets_for_managed_processes(
            &process_infos,
            &managed,
            preserve_surface_ids,
        );
        let (signaled, errors) = terminate_processes(&targets);
        summary.processes_signaled = signaled;
        summary.process_signal_errors = errors;
    }

    summary.sockets_removed = cleanup_session_sockets(runtime_dir, preserve_surface_ids)?;
    Ok(summary)
}

/// Best-effort cleanup for one explicitly closed or restarted surface. Unlike
/// UI process shutdown, this is a user-visible request to discard the detached
/// terminal session, so ForkTTY removes the target broker socket and terminates
/// only the matching managed broker process tree.
pub fn cleanup_managed_session(
    runtime_dir: &Path,
    surface_id: &str,
) -> io::Result<PtyPersistenceCleanup> {
    let mut summary = PtyPersistenceCleanup::default();

    #[cfg(target_os = "linux")]
    {
        let process_infos = linux_process_infos()?;
        let managed = managed_dtach_processes_from_infos(&process_infos, runtime_dir)
            .into_iter()
            .filter(|process| process.socket.surface_id == surface_id)
            .collect::<Vec<_>>();
        let targets =
            termination_targets_for_managed_processes(&process_infos, &managed, &BTreeSet::new());
        let (signaled, errors) = terminate_processes(&targets);
        summary.processes_signaled = signaled;
        summary.process_signal_errors = errors;
    }

    summary.sockets_removed = usize::from(cleanup_session_socket(runtime_dir, surface_id)?);
    Ok(summary)
}

fn cleanup_session_sockets(
    runtime_dir: &Path,
    preserve_surface_ids: &BTreeSet<String>,
) -> io::Result<usize> {
    let session_dir = runtime_dir.join(PTY_SESSION_DIR_NAME);
    let entries = match std::fs::read_dir(&session_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err),
    };
    let mut removed = 0usize;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();
        let Some(socket) = managed_socket_path(&path, &session_dir) else {
            continue;
        };
        if preserve_surface_ids.contains(&socket.surface_id) {
            continue;
        }
        match std::fs::remove_file(&socket.socket_path) {
            Ok(()) => removed += 1,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    Ok(removed)
}

fn managed_dtach_socket_from_cmdline(
    args: &[OsString],
    runtime_dir: &Path,
) -> Option<ManagedDtachSocket> {
    let first = args.first()?;
    if Path::new(first).file_name() != Some(OsStr::new("dtach")) {
        return None;
    }
    let socket_arg = args
        .windows(2)
        .find(|window| window[0] == OsStr::new("-A"))
        .map(|window| PathBuf::from(&window[1]))?;
    managed_socket_path(&socket_arg, &runtime_dir.join(PTY_SESSION_DIR_NAME))
}

fn managed_socket_path(path: &Path, session_dir: &Path) -> Option<ManagedDtachSocket> {
    if path.parent()? != session_dir {
        return None;
    }
    let file_name = path.file_name()?.to_str()?;
    let surface_id = file_name.strip_suffix(".sock")?;
    if !is_safe_surface_id(surface_id) {
        return None;
    }
    Some(ManagedDtachSocket {
        socket_path: path.to_path_buf(),
        surface_id: surface_id.to_string(),
    })
}

#[cfg(target_os = "linux")]
fn managed_dtach_processes_from_infos(
    infos: &[LinuxProcessInfo],
    runtime_dir: &Path,
) -> Vec<ManagedDtachProcess> {
    infos
        .iter()
        .filter_map(|info| {
            managed_dtach_socket_from_cmdline(&info.args, runtime_dir).map(|socket| {
                ManagedDtachProcess {
                    pid: info.pid,
                    socket,
                }
            })
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn linux_process_infos() -> io::Result<Vec<LinuxProcessInfo>> {
    let mut infos = Vec::new();
    let own_pid = unsafe { libc::getpid() };
    let entries = std::fs::read_dir("/proc")?;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<libc::pid_t>().ok())
        else {
            continue;
        };
        if pid <= 0 || pid == own_pid {
            continue;
        }
        let cmdline = match std::fs::read(entry.path().join("cmdline")) {
            Ok(cmdline) => cmdline,
            Err(_) => continue,
        };
        let args = proc_cmdline_args(&cmdline);
        let ppid = read_proc_ppid(&entry.path().join("status")).unwrap_or(0);
        infos.push(LinuxProcessInfo { pid, ppid, args });
    }
    Ok(infos)
}

#[cfg(target_os = "linux")]
fn read_proc_ppid(status_path: &Path) -> io::Result<libc::pid_t> {
    let status = std::fs::read_to_string(status_path)?;
    for line in status.lines() {
        if let Some(ppid) = line.strip_prefix("PPid:") {
            return ppid
                .trim()
                .parse::<libc::pid_t>()
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err));
        }
    }
    Ok(0)
}

#[cfg(target_os = "linux")]
fn proc_cmdline_args(bytes: &[u8]) -> Vec<OsString> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|arg| !arg.is_empty())
        .map(|arg| OsStr::from_bytes(arg).to_os_string())
        .collect()
}

#[cfg(target_os = "linux")]
fn termination_targets_for_managed_processes(
    infos: &[LinuxProcessInfo],
    managed: &[ManagedDtachProcess],
    preserve_surface_ids: &BTreeSet<String>,
) -> BTreeSet<libc::pid_t> {
    let mut children_by_parent: BTreeMap<libc::pid_t, Vec<libc::pid_t>> = BTreeMap::new();
    for info in infos {
        children_by_parent
            .entry(info.ppid)
            .or_default()
            .push(info.pid);
    }
    let mut targets = BTreeSet::new();
    for process in managed {
        if preserve_surface_ids.contains(&process.socket.surface_id) {
            continue;
        }
        let mut stack = vec![process.pid];
        while let Some(pid) = stack.pop() {
            if targets.insert(pid) {
                if let Some(children) = children_by_parent.get(&pid) {
                    stack.extend(children);
                }
            }
        }
    }
    targets
}

#[cfg(target_os = "linux")]
fn terminate_processes(pids: &BTreeSet<libc::pid_t>) -> (usize, usize) {
    let (mut signaled, mut errors) = signal_processes(pids, libc::SIGTERM);
    if !pids.is_empty() {
        std::thread::sleep(Duration::from_millis(150));
    }
    let remaining = pids
        .iter()
        .copied()
        .filter(|pid| process_exists(*pid))
        .collect::<BTreeSet<_>>();
    if !remaining.is_empty() {
        let (kill_signaled, kill_errors) = signal_processes(&remaining, libc::SIGKILL);
        signaled += kill_signaled;
        errors += kill_errors;
    }
    (signaled, errors)
}

#[cfg(target_os = "linux")]
fn signal_processes(pids: &BTreeSet<libc::pid_t>, signal: libc::c_int) -> (usize, usize) {
    let mut signaled = 0usize;
    let mut errors = 0usize;
    for pid in pids.iter().rev().copied() {
        match signal_process(pid, signal) {
            Ok(true) => signaled += 1,
            Ok(false) => {}
            Err(_) => errors += 1,
        }
    }
    (signaled, errors)
}

#[cfg(target_os = "linux")]
fn signal_process(pid: libc::pid_t, signal: libc::c_int) -> io::Result<bool> {
    let rc = unsafe { libc::kill(pid, signal) };
    if rc == 0 {
        return Ok(true);
    }
    let err = io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::ESRCH) {
        return Ok(false);
    }
    Err(err)
}

#[cfg(target_os = "linux")]
fn process_exists(pid: libc::pid_t) -> bool {
    signal_process(pid, 0).unwrap_or(true)
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
    use std::ffi::OsString;
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

    #[cfg(unix)]
    #[test]
    fn managed_dtach_socket_from_cmdline_matches_only_forktty_session_sockets() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join(PTY_SESSION_DIR_NAME);
        let socket = session_dir.join("surface-3.sock");
        let args = vec![
            OsString::from("/usr/bin/dtach"),
            OsString::from("-A"),
            socket.clone().into_os_string(),
            OsString::from("-E"),
            OsString::from("-z"),
            OsString::from("/usr/bin/zsh"),
        ];

        let matched = managed_dtach_socket_from_cmdline(&args, dir.path()).unwrap();
        assert_eq!(matched.socket_path, socket);
        assert_eq!(matched.surface_id, "surface-3");

        let outside = vec![
            OsString::from("/usr/bin/dtach"),
            OsString::from("-A"),
            dir.path().join("outside.sock").into_os_string(),
            OsString::from("/usr/bin/zsh"),
        ];
        assert!(managed_dtach_socket_from_cmdline(&outside, dir.path()).is_none());

        let unsafe_surface = vec![
            OsString::from("/usr/bin/dtach"),
            OsString::from("-A"),
            session_dir.join("../surface-4.sock").into_os_string(),
            OsString::from("/usr/bin/zsh"),
        ];
        assert!(managed_dtach_socket_from_cmdline(&unsafe_surface, dir.path()).is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn termination_targets_include_managed_dtach_descendants_only() {
        let runtime = Path::new("/run/user/1000");
        let managed_socket = PathBuf::from("/run/user/1000/forktty-pty/surface-stale.sock");
        let live_socket = PathBuf::from("/run/user/1000/forktty-pty/surface-live.sock");
        let outside_socket = PathBuf::from("/run/user/1000/other/surface-outside.sock");
        let infos = vec![
            LinuxProcessInfo {
                pid: 10,
                ppid: 1,
                args: vec![
                    OsString::from("/usr/bin/dtach"),
                    OsString::from("-A"),
                    managed_socket.into_os_string(),
                    OsString::from("/usr/bin/zsh"),
                ],
            },
            LinuxProcessInfo {
                pid: 11,
                ppid: 10,
                args: vec![OsString::from("/usr/bin/zsh")],
            },
            LinuxProcessInfo {
                pid: 12,
                ppid: 11,
                args: vec![OsString::from("/usr/bin/sleep"), OsString::from("60")],
            },
            LinuxProcessInfo {
                pid: 20,
                ppid: 1,
                args: vec![
                    OsString::from("/usr/bin/dtach"),
                    OsString::from("-A"),
                    live_socket.into_os_string(),
                    OsString::from("/usr/bin/zsh"),
                ],
            },
            LinuxProcessInfo {
                pid: 21,
                ppid: 20,
                args: vec![OsString::from("/usr/bin/zsh")],
            },
            LinuxProcessInfo {
                pid: 30,
                ppid: 1,
                args: vec![
                    OsString::from("/usr/bin/dtach"),
                    OsString::from("-A"),
                    outside_socket.into_os_string(),
                    OsString::from("/usr/bin/zsh"),
                ],
            },
            LinuxProcessInfo {
                pid: 31,
                ppid: 30,
                args: vec![OsString::from("/usr/bin/zsh")],
            },
        ];

        let managed = managed_dtach_processes_from_infos(&infos, runtime);
        let preserve = BTreeSet::from(["surface-live".to_string()]);
        let targets = termination_targets_for_managed_processes(&infos, &managed, &preserve);

        assert_eq!(targets, BTreeSet::from([10, 11, 12]));
    }

    #[test]
    fn cleanup_managed_sessions_removes_stale_sockets_but_preserves_live_surfaces() {
        let dir = tempfile::tempdir().unwrap();
        let stale = session_socket_path(dir.path(), "surface-stale").unwrap();
        let live = session_socket_path(dir.path(), "surface-live").unwrap();
        ensure_private_session_dir(&stale).unwrap();
        fs::write(&stale, b"stale").unwrap();
        fs::write(&live, b"live").unwrap();

        let preserve = BTreeSet::from(["surface-live".to_string()]);
        let summary = cleanup_managed_sessions(dir.path(), &preserve).unwrap();

        assert_eq!(summary.sockets_removed, 1);
        assert!(!stale.exists());
        assert!(live.exists());
    }

    #[test]
    fn cleanup_managed_session_removes_only_target_socket() {
        let dir = tempfile::tempdir().unwrap();
        let target = session_socket_path(dir.path(), "surface-target").unwrap();
        let other = session_socket_path(dir.path(), "surface-other").unwrap();
        ensure_private_session_dir(&target).unwrap();
        fs::write(&target, b"target").unwrap();
        fs::write(&other, b"other").unwrap();

        let summary = cleanup_managed_session(dir.path(), "surface-target").unwrap();

        assert_eq!(summary.sockets_removed, 1);
        assert!(!target.exists());
        assert!(other.exists());
    }
}
