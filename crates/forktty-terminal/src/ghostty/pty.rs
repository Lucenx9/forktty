use crate::SpawnRequest;
use nix::{
    fcntl::{fcntl, FcntlArg, OFlag},
    pty::{openpty, Winsize},
    unistd::setsid,
};
use std::{
    fs::File,
    io::{self, Read, Write},
    os::fd::{AsRawFd, OwnedFd},
    os::unix::process::CommandExt,
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

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
        let pty = openpty(Some(&winsize), None).map_err(io_error)?;
        set_nonblocking(&pty.master)?;
        let slave_fd = pty.slave.as_raw_fd();
        let slave_stdin = duplicate_fd(&pty.slave)?;
        let slave_stdout = duplicate_fd(&pty.slave)?;
        let slave_stderr = pty.slave;

        let argv = crate::spawn::child_argv(request, &crate::spawn::appimage_runtime_env_keys());
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
        let master = File::from(pty.master);
        Ok(Self {
            master,
            child,
            size,
        })
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.master.write_all(bytes)
    }

    pub fn read_available(&mut self) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match self.master.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
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
        Ok(out)
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
    let raw = nix::unistd::dup(fd).map_err(io_error)?;
    Ok(raw)
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
    use std::{path::PathBuf, time::Duration};

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
