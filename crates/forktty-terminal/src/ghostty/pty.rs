use crate::SpawnRequest;
use nix::{
    fcntl::{fcntl, FcntlArg, OFlag},
    poll::{poll, PollFd, PollFlags, PollTimeout},
    pty::Winsize,
    unistd::setsid,
};
use std::{
    fs::File,
    io::{self, Read, Write},
    os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
    os::unix::process::CommandExt,
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

/// Overall cap on a single `write_all`: long enough for a slow consumer of a
/// huge paste, short enough that a child ignoring its tty cannot wedge us.
const WRITE_ALL_TIMEOUT: Duration = Duration::from_secs(10);

/// Per-call cap on `read_available`: bounds main-loop time per pump tick
/// under a flooding child (the pump runs every ~16ms and picks up the rest).
const READ_AVAILABLE_BYTE_CAP: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug)]
pub struct PtySession {
    master: File,
    child: Child,
    size: PtySize,
}

impl PtySession {
    pub fn spawn(request: &SpawnRequest, size: PtySize) -> io::Result<Self> {
        let winsize = Winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // Master and slave are opened with O_CLOEXEC atomically: openpty()
        // cannot set the flag at open time, and setting it afterwards leaves
        // a window where a fork on another thread (worktree hooks,
        // notification commands) inherits the fd, keeping the pty alive
        // after we drop it and leaking one fd per process.
        let master_pt = nix::pty::posix_openpt(OFlag::O_RDWR | OFlag::O_NOCTTY | OFlag::O_CLOEXEC)
            .map_err(io_error)?;
        nix::pty::grantpt(&master_pt).map_err(io_error)?;
        nix::pty::unlockpt(&master_pt).map_err(io_error)?;
        let slave_path = nix::pty::ptsname_r(&master_pt).map_err(io_error)?;
        let master = OwnedFd::from(master_pt);
        let slave = nix::fcntl::open(
            slave_path.as_str(),
            OFlag::O_RDWR | OFlag::O_NOCTTY | OFlag::O_CLOEXEC,
            nix::sys::stat::Mode::empty(),
        )
        .map_err(io_error)?;
        unsafe {
            tiocswinsz(master.as_raw_fd(), &winsize)
                .map_err(|err| io::Error::from_raw_os_error(err as i32))?;
        }
        set_nonblocking(&master)?;
        let slave_fd = slave.as_raw_fd();
        let slave_stdin = duplicate_fd(&slave)?;
        let slave_stdout = duplicate_fd(&slave)?;
        let slave_stderr = slave;

        let argv = crate::spawn::child_argv(request, crate::spawn::appimage_runtime_env_keys());
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty terminal argv"))?;
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(crate::spawn::child_cwd(request))
            .env_clear()
            .envs(parse_env(crate::spawn::child_environment(request)))
            .stdin(Stdio::from(File::from(slave_stdin)))
            .stdout(Stdio::from(File::from(slave_stdout)))
            .stderr(Stdio::from(File::from(slave_stderr)));
        unsafe {
            command.pre_exec(move || {
                setsid().map_err(io_error)?;
                // Acquire the slave pty as the controlling terminal so programs
                // that open /dev/tty (fzf, less, ssh/sudo password prompts, ...)
                // work regardless of whether the child shell sets one up itself.
                tiocsctty(slave_fd, 0).map_err(io_error)?;
                Ok(())
            });
        }
        let child = command.spawn()?;
        let master = File::from(master);
        Ok(Self {
            master,
            child,
            size,
        })
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        // The master fd is O_NONBLOCK, so std's write_all would fail with
        // WouldBlock after a partial write once the kernel pty buffer fills
        // (large pastes). Drain manually, polling for writability instead.
        // The overall deadline is a generous cap so a child that never reads
        // its tty cannot wedge the caller forever.
        let deadline = Instant::now() + WRITE_ALL_TIMEOUT;
        let mut written = 0;
        while written < bytes.len() {
            match self.master.write(&bytes[written..]) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "pty master accepted no bytes",
                    ));
                }
                Ok(n) => written += n,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    poll_master_write_ready(self.master.as_fd(), deadline)?;
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    pub fn read_available(&mut self) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match self.master.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    out.extend_from_slice(&buf[..n]);
                    // Bounds main-loop time per pump tick under a flooding
                    // child; the next tick picks up the rest.
                    if out.len() >= READ_AVAILABLE_BYTE_CAP {
                        break;
                    }
                }
                Err(err)
                    if err.kind() == io::ErrorKind::WouldBlock
                        || err.raw_os_error() == Some(libc::EIO) =>
                {
                    break;
                }
                Err(err) => return Err(err),
            }
        }
        Ok(out)
    }

    pub fn read_until(&mut self, needle: &[u8], timeout: Duration) -> io::Result<Vec<u8>> {
        if needle.is_empty() {
            // windows(0) panics; an empty needle is trivially found.
            return self.read_available();
        }
        let deadline = Instant::now() + timeout;
        let mut out = Vec::new();
        while Instant::now() < deadline {
            out.extend(self.read_available()?);
            if out.windows(needle.len()).any(|window| window == needle) {
                return Ok(out);
            }
            if self.child.try_wait()?.is_some() {
                out.extend(self.read_available()?);
                return Ok(out);
            }
            thread::sleep(Duration::from_millis(10));
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "terminal output did not contain expected bytes before timeout",
        ))
    }

    pub fn resize(&mut self, size: PtySize) -> io::Result<()> {
        let winsize = libc::winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        unsafe {
            tiocswinsz(self.master.as_raw_fd(), &winsize)
                .map_err(|err| io::Error::from_raw_os_error(err as i32))?;
        }
        self.size = size;
        Ok(())
    }

    pub fn size(&self) -> PtySize {
        self.size
    }

    pub fn child_id(&self) -> u32 {
        self.child.id()
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    pub fn wait_timeout(&mut self, timeout: Duration) -> io::Result<ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "terminal child did not exit before timeout",
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

nix::ioctl_write_ptr_bad!(tiocswinsz, libc::TIOCSWINSZ, libc::winsize);
nix::ioctl_write_int_bad!(tiocsctty, libc::TIOCSCTTY);

fn set_nonblocking(fd: &OwnedFd) -> io::Result<()> {
    let flags = OFlag::from_bits_truncate(fcntl(fd, FcntlArg::F_GETFL).map_err(io_error)?);
    fcntl(fd, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).map_err(io_error)?;
    Ok(())
}

fn duplicate_fd(fd: &OwnedFd) -> io::Result<OwnedFd> {
    // F_DUPFD_CLOEXEC, not dup(): the duplicate must not be inheritable by a
    // fork racing on another thread; the spawned child's dup2 onto
    // stdin/stdout clears the flag for the intended process only.
    let raw = fcntl(fd, FcntlArg::F_DUPFD_CLOEXEC(0)).map_err(io_error)?;
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

fn poll_master_write_ready(fd: BorrowedFd<'_>, deadline: Instant) -> io::Result<()> {
    loop {
        if Instant::now() >= deadline {
            return Err(write_all_timeout_error());
        }
        let mut fds = [PollFd::new(fd, PollFlags::POLLOUT)];
        match poll(&mut fds, PollTimeout::from(100u16)) {
            Ok(0) => {}
            Ok(_) => return Ok(()),
            Err(nix::Error::EINTR) => {}
            Err(err) => return Err(io_error(err)),
        }
    }
}

fn write_all_timeout_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        "terminal child did not drain pty input before timeout",
    )
}

fn parse_env(env: Vec<String>) -> impl Iterator<Item = (String, String)> {
    env.into_iter().filter_map(|entry| {
        let (key, value) = entry.split_once('=')?;
        Some((key.to_string(), value.to_string()))
    })
}

fn io_error(err: nix::Error) -> io::Error {
    io::Error::from_raw_os_error(err as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::Duration,
    };

    fn test_spawn_request_for_shell(shell: &str) -> SpawnRequest {
        SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: shell.to_string(),
            args: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        }
    }

    #[test]
    fn pty_spawns_controlled_command_and_reads_output() {
        let request =
            test_spawn_request_for_shell("/bin/sh").with_args(["-lc", "printf forktty-pty"]);
        let mut session = PtySession::spawn(&request, PtySize { cols: 80, rows: 24 }).unwrap();

        let output = session
            .read_until(b"forktty-pty", Duration::from_secs(2))
            .unwrap();

        assert!(output
            .windows("forktty-pty".len())
            .any(|window| window == b"forktty-pty"));
        assert_eq!(
            session.wait_timeout(Duration::from_secs(2)).unwrap().code(),
            Some(0)
        );
    }

    #[test]
    fn pty_child_acquires_controlling_terminal() {
        // fzf, less, ssh password prompts, ... open /dev/tty directly, which only
        // works when the child owns the pty as its controlling terminal. Use a
        // non-shell child (sleep): unlike bash, it never re-opens its tty by name,
        // so it has a controlling terminal only if the pty layer set one
        // (setsid + TIOCSCTTY). We read the controlling-tty device from
        // /proc/<pid>/stat (field `tty_nr`, 0 when there is none).
        let request = test_spawn_request_for_shell("/bin/sleep").with_args(["2"]);
        let session = PtySession::spawn(&request, PtySize { cols: 80, rows: 24 }).unwrap();
        let pid = session.child.id();

        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        // tty_nr is the 5th field after (comm): state, ppid, pgrp, session, tty_nr.
        let tty_nr: i64 = forktty_core::ports::stat_field_after_comm(&stat, 4)
            .expect("stat has tty_nr field")
            .parse()
            .expect("tty_nr is an integer");

        assert_ne!(
            tty_nr, 0,
            "child has no controlling terminal (tty_nr=0); /dev/tty would fail with ENXIO"
        );
    }

    #[test]
    fn pty_master_fd_is_not_inherited_by_child() {
        // Without CLOEXEC on the master, every child (and its descendants)
        // keeps the pty master open. /proc/<pid>/fd links of a pty master
        // point at /dev/ptmx.
        let request = test_spawn_request_for_shell("/bin/sleep").with_args(["2"]);
        let session = PtySession::spawn(&request, PtySize { cols: 80, rows: 24 }).unwrap();
        let pid = session.child.id();

        // Wait for exec to complete (CLOEXEC only takes effect at exec time).
        let deadline = Instant::now() + Duration::from_secs(2);
        while std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .map(|comm| comm.trim() != "sleep")
            .unwrap_or(true)
        {
            assert!(Instant::now() < deadline, "child never exec'd /bin/sleep");
            thread::sleep(Duration::from_millis(10));
        }

        let leaked: Vec<_> = std::fs::read_dir(format!("/proc/{pid}/fd"))
            .unwrap()
            .filter_map(|entry| std::fs::read_link(entry.ok()?.path()).ok())
            .filter(|target| target.to_string_lossy().contains("ptmx"))
            .collect();
        assert!(leaked.is_empty(), "child inherited pty master: {leaked:?}");
    }

    #[test]
    fn pty_write_all_delivers_payloads_larger_than_the_kernel_buffer() {
        let request = test_spawn_request_for_shell("/bin/sh").with_args(["-c", "wc -c"]);
        let mut session = PtySession::spawn(&request, PtySize { cols: 80, rows: 24 }).unwrap();

        // Echo would mirror every input byte into the slave->master buffer,
        // which nothing drains while write_all runs; turn it off so the only
        // output is the byte count wc prints at EOF.
        let mut termios = nix::sys::termios::tcgetattr(&session.master).unwrap();
        termios
            .local_flags
            .remove(nix::sys::termios::LocalFlags::ECHO);
        nix::sys::termios::tcsetattr(
            &session.master,
            nix::sys::termios::SetArg::TCSANOW,
            &termios,
        )
        .unwrap();

        // 256 KiB of newline-terminated lines: far beyond the kernel pty
        // buffer, so the nonblocking master hits WouldBlock mid-write (std's
        // write_all used to error there, silently truncating large pastes).
        let line = b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcd\n";
        let mut payload = Vec::new();
        while payload.len() < 256 * 1024 {
            payload.extend_from_slice(line);
        }

        session.write_all(&payload).unwrap();
        // VEOF at the start of a line delivers EOF: wc prints the count.
        session.write_all(b"\x04").unwrap();

        let expected = payload.len().to_string();
        let output = session
            .read_until(expected.as_bytes(), Duration::from_secs(10))
            .unwrap();
        let output = String::from_utf8_lossy(&output);
        assert!(
            output.contains(&expected),
            "child saw a truncated paste: wc reported {output:?}, expected {expected} bytes"
        );
    }

    #[test]
    fn poll_master_write_ready_times_out_when_fd_stays_blocked() {
        let (_read_fd, write_fd) = nix::unistd::pipe().unwrap();
        set_nonblocking(&write_fd).unwrap();

        let block = vec![0u8; 8192];
        loop {
            match nix::unistd::write(&write_fd, &block) {
                Ok(_) => {}
                Err(nix::Error::EAGAIN) => break,
                Err(err) => panic!("unexpected pipe fill error: {err}"),
            }
        }

        let err = poll_master_write_ready(
            write_fd.as_fd(),
            Instant::now() + Duration::from_millis(150),
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn pty_read_until_times_out_when_needle_missing() {
        let request =
            test_spawn_request_for_shell("/bin/sh").with_args(["-lc", "printf partial; sleep 1"]);
        let mut session = PtySession::spawn(&request, PtySize { cols: 80, rows: 24 }).unwrap();

        let err = session
            .read_until(b"missing", Duration::from_millis(50))
            .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn pty_write_all_retries_poll_interrupted_by_signal() {
        extern "C" fn ignore_signal(_: libc::c_int) {}

        let action = nix::sys::signal::SigAction::new(
            nix::sys::signal::SigHandler::Handler(ignore_signal),
            nix::sys::signal::SaFlags::empty(),
            nix::sys::signal::SigSet::empty(),
        );
        let old_action =
            unsafe { nix::sys::signal::sigaction(nix::sys::signal::Signal::SIGUSR1, &action) }
                .unwrap();
        struct RestoreSignal(nix::sys::signal::SigAction);
        impl Drop for RestoreSignal {
            fn drop(&mut self) {
                unsafe {
                    let _ = nix::sys::signal::sigaction(nix::sys::signal::Signal::SIGUSR1, &self.0);
                }
            }
        }
        let _restore_signal = RestoreSignal(old_action);

        let request = test_spawn_request_for_shell("/bin/sh").with_args(["-c", "sleep 0.5; wc -c"]);
        let mut session = PtySession::spawn(&request, PtySize { cols: 80, rows: 24 }).unwrap();
        let mut termios = nix::sys::termios::tcgetattr(&session.master).unwrap();
        termios
            .local_flags
            .remove(nix::sys::termios::LocalFlags::ECHO);
        nix::sys::termios::tcsetattr(
            &session.master,
            nix::sys::termios::SetArg::TCSANOW,
            &termios,
        )
        .unwrap();

        let target_thread = unsafe { libc::pthread_self() as usize };
        let done = Arc::new(AtomicBool::new(false));
        let done_for_signaler = done.clone();
        let signaler = thread::spawn(move || {
            while !done_for_signaler.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(10));
                unsafe {
                    libc::pthread_kill(target_thread as libc::pthread_t, libc::SIGUSR1);
                }
            }
        });

        let line = b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcd\n";
        let mut payload = Vec::new();
        while payload.len() < 256 * 1024 {
            payload.extend_from_slice(line);
        }

        let write_result = session.write_all(&payload);
        done.store(true, Ordering::SeqCst);
        signaler.join().unwrap();
        write_result.unwrap();
        session.write_all(b"\x04").unwrap();

        let expected = payload.len().to_string();
        let output = session
            .read_until(expected.as_bytes(), Duration::from_secs(10))
            .unwrap();
        let output = String::from_utf8_lossy(&output);
        assert!(
            output.contains(&expected),
            "child saw a truncated paste after EINTR: wc reported {output:?}, expected {expected} bytes"
        );
    }

    #[test]
    fn pty_resize_tracks_requested_size() {
        let request = test_spawn_request_for_shell("/bin/sh").with_args(["-lc", "sleep 1"]);
        let mut session = PtySession::spawn(&request, PtySize { cols: 80, rows: 24 }).unwrap();

        session
            .resize(PtySize {
                cols: 120,
                rows: 40,
            })
            .unwrap();

        assert_eq!(
            session.size(),
            PtySize {
                cols: 120,
                rows: 40
            }
        );
    }
}
