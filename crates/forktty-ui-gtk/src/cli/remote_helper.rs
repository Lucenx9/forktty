//! Native remote-helper command parsing and stdio PTY relay.

use super::{unknown_argument, RemoteHelperCommand, VERSION};
use serde_json::json;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::PathBuf;

pub(super) fn parse_remote_helper_command(
    args: &[OsString],
) -> Result<RemoteHelperCommand, String> {
    let Some(command) = args.first() else {
        return Err("remote-helper requires a subcommand: hello or pty".to_string());
    };
    let Some(command) = command.to_str() else {
        return Err(unknown_argument("<non-utf8>"));
    };
    match command {
        "hello" => {
            if args.len() > 1 {
                let extra = args[1].to_str().unwrap_or("<non-utf8>");
                return Err(format!("remote-helper hello: unexpected argument {extra}"));
            }
            Ok(RemoteHelperCommand::Hello)
        }
        "pty" => parse_remote_helper_pty_args(&args[1..]),
        other => Err(unknown_argument(other)),
    }
}

fn parse_remote_helper_pty_args(args: &[OsString]) -> Result<RemoteHelperCommand, String> {
    if args.first().and_then(|arg| arg.to_str()) != Some("--") {
        return Err("remote-helper pty requires -- <program> [args...]".to_string());
    }
    let mut argv = Vec::new();
    for arg in &args[1..] {
        let Some(arg) = arg.to_str() else {
            return Err(unknown_argument("<non-utf8>"));
        };
        argv.push(arg.to_string());
    }
    if argv.is_empty() {
        return Err("remote-helper pty requires -- <program> [args...]".to_string());
    }
    Ok(RemoteHelperCommand::Pty { argv })
}

pub fn print_remote_helper_hello() {
    println!("{}", remote_helper_hello_json());
}

#[cfg(feature = "gtk-ghostty")]
pub fn run_remote_helper_pty(argv: Vec<String>) -> i32 {
    match run_remote_helper_pty_inner(argv) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("forktty remote-helper pty: {err}");
            1
        }
    }
}

#[cfg(not(feature = "gtk-ghostty"))]
pub fn run_remote_helper_pty(_argv: Vec<String>) -> i32 {
    eprintln!("forktty remote-helper pty requires the gtk-ghostty feature");
    1
}

#[cfg(feature = "gtk-ghostty")]
fn run_remote_helper_pty_inner(argv: Vec<String>) -> io::Result<i32> {
    use forktty_terminal::ghostty::pty::{PtySession, PtySize};

    let cwd = std::env::current_dir()?;
    let request = remote_helper_pty_request(argv, cwd)?;
    let session = PtySession::spawn(&request, PtySize { cols: 80, rows: 24 })?;
    // `Stdin` (not `StdinLock`) because the reader crosses into the relay
    // thread and the lock guard is not `Send`; it locks per read call.
    relay_pty_stdio(session, io::stdin(), io::stdout().lock())
}

#[cfg(feature = "gtk-ghostty")]
trait PtyRelaySession {
    fn try_write(&mut self, bytes: &[u8]) -> io::Result<usize>;
    fn eof_bytes(&self) -> io::Result<[u8; 2]>;
    fn read_available(&mut self) -> io::Result<Vec<u8>>;
    fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>>;
}

#[cfg(feature = "gtk-ghostty")]
impl PtyRelaySession for forktty_terminal::ghostty::pty::PtySession {
    fn try_write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.try_write(bytes)
    }

    fn eof_bytes(&self) -> io::Result<[u8; 2]> {
        self.eof_bytes()
    }

    fn read_available(&mut self) -> io::Result<Vec<u8>> {
        self.read_available()
    }

    fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        self.try_wait()
    }
}

/// Relay stdin/stdout to the PTY session until the child exits.
///
/// Generic over the stdio endpoints so tests can drive the relay with
/// in-memory streams; production passes the process stdin/stdout locks.
#[cfg(feature = "gtk-ghostty")]
fn relay_pty_stdio<S, R, W>(mut session: S, stdin: R, mut stdout: W) -> io::Result<i32>
where
    S: PtyRelaySession,
    R: std::io::Read + Send + 'static,
    W: std::io::Write,
{
    // Keep the stdin relay bounded so a child that stops draining its PTY
    // applies backpressure to the helper instead of letting queued input grow
    // without limit.
    enum StdinRelayEvent {
        Data(Vec<u8>),
        Eof,
        Error(io::Error),
    }

    let (tx, rx) = std::sync::mpsc::sync_channel::<StdinRelayEvent>(16);
    std::thread::spawn(move || {
        let mut stdin = stdin;
        let mut buf = [0u8; 8192];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => {
                    let _ = tx.send(StdinRelayEvent::Eof);
                    break;
                }
                Ok(n) if tx.send(StdinRelayEvent::Data(buf[..n].to_vec())).is_err() => break,
                Ok(_) => {}
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => {
                    let _ = tx.send(StdinRelayEvent::Error(err));
                    break;
                }
            }
        }
    });
    // Input the PTY has not accepted yet. Writes must never wait for the
    // child to drain its input: while the input buffer is full the child may
    // itself be blocked writing output, and only the output pump below can
    // unwedge it — a waiting write (the old `write_all` here) stalls both
    // directions until the session dies on the write timeout.
    let mut pending: Vec<u8> = Vec::new();
    let mut pending_offset = 0;
    let mut stdin_eof = false;
    let mut eof_queued = false;
    loop {
        // Pump queued stdin into the PTY until it stops accepting or the
        // relay has nothing buffered. Self-limiting: a child that stops
        // reading fills the PTY buffer and try_write returns 0.
        loop {
            while pending_offset < pending.len() {
                let written = session.try_write(&pending[pending_offset..])?;
                if written == 0 {
                    break;
                }
                pending_offset += written;
            }
            if pending_offset < pending.len() {
                break;
            }
            pending.clear();
            pending_offset = 0;
            if eof_queued {
                break;
            }
            if stdin_eof {
                // Queue EOF only after everything before it reached the
                // child, then pump it through the same nonblocking path so a
                // full input buffer cannot stop output draining.
                pending.extend_from_slice(&session.eof_bytes()?);
                eof_queued = true;
                continue;
            }
            match rx.try_recv() {
                Ok(StdinRelayEvent::Data(bytes)) => pending = bytes,
                Ok(StdinRelayEvent::Eof) => stdin_eof = true,
                Ok(StdinRelayEvent::Error(err)) => return Err(err),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "stdin relay disconnected before reporting EOF",
                    ));
                }
            }
        }
        write_pty_output(&mut session, &mut stdout)?;
        if let Some(status) = session.try_wait()? {
            write_pty_output(&mut session, &mut stdout)?;
            return Ok(exit_code_from_status(status));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Map a child exit status to a shell-style exit code, preserving
/// signal terminations as `128 + signal` (e.g. SIGTERM → 143) instead of
/// collapsing every signal death to a bare `1`.
#[cfg(feature = "gtk-ghostty")]
fn exit_code_from_status(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    if let Some(code) = status.code() {
        code
    } else if let Some(signal) = status.signal() {
        128 + signal
    } else {
        1
    }
}

#[cfg(feature = "gtk-ghostty")]
fn write_pty_output<S: PtyRelaySession, W: std::io::Write>(
    session: &mut S,
    stdout: &mut W,
) -> io::Result<()> {
    let output = session.read_available()?;
    if !output.is_empty() {
        stdout.write_all(&output)?;
        stdout.flush()?;
    }
    Ok(())
}

pub(super) fn remote_helper_pty_request(
    argv: Vec<String>,
    cwd: PathBuf,
) -> io::Result<forktty_terminal::SpawnRequest> {
    let mut argv = argv.into_iter();
    let shell = argv.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "remote-helper pty requires a program",
        )
    })?;
    Ok(forktty_terminal::SpawnRequest {
        surface_id: "remote-helper".to_string(),
        workspace_id: "remote-helper".to_string(),
        shell,
        args: argv.collect(),
        cwd,
        socket_path: PathBuf::new(),
        extra_env: Vec::new(),
        eligible_for_pty_persistence: false,
    })
}

fn remote_helper_hello_json() -> String {
    remote_helper_hello_payload(std::env::current_dir().ok(), local_hostname()).to_string()
}

pub(super) fn remote_helper_hello_payload(
    cwd: Option<PathBuf>,
    hostname: Option<String>,
) -> serde_json::Value {
    json!({
        "schema": 1,
        "kind": "forktty.remote.hello",
        "protocol": "forktty-remote-stdio",
        "protocol_version": 1,
        "transport": "stdio",
        "version": VERSION,
        "cwd": cwd.map(|path| path.display().to_string()),
        "hostname": hostname,
        "platform": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
        "capabilities": ["hello", "pty"],
    })
}

fn local_hostname() -> Option<String> {
    std::env::var("HOSTNAME")
        .ok()
        .and_then(non_empty_trimmed)
        .or_else(|| {
            fs::read_to_string("/etc/hostname")
                .ok()
                .and_then(non_empty_trimmed)
        })
}

fn non_empty_trimmed(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

#[cfg(all(test, feature = "gtk-ghostty"))]
mod tests {
    use super::{
        exit_code_from_status, relay_pty_stdio, remote_helper_pty_request, PtyRelaySession,
    };
    use forktty_terminal::ghostty::pty::{PtySession, PtySize};
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;
    use std::time::{Duration, Instant};

    #[test]
    fn relay_survives_full_duplex_backpressure() {
        // The child floods hundreds of KiB of output before it starts reading
        // stdin, while the relay has a large paste queued: the PTY input
        // buffer fills, and a relay that waits for the child to drain it
        // stops pumping output, so the child in turn blocks writing — both
        // directions back up and the session dies on the write timeout
        // instead of completing.
        let request = remote_helper_pty_request(
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "seq 1 100000; exec cat >/dev/null".to_string(),
            ],
            std::env::current_dir().unwrap(),
        )
        .unwrap();
        let session = PtySession::spawn(&request, PtySize { cols: 80, rows: 24 }).unwrap();

        // Newline-terminated lines: the canonical-mode PTY completes each
        // line into the input queue, so the queue genuinely fills.
        let line = b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcd\n";
        let payload = line.repeat(4096); // 256 KiB, far beyond the PTY buffer

        let started = Instant::now();
        let code = relay_pty_stdio(session, std::io::Cursor::new(payload), std::io::sink())
            .expect("relay must complete a legitimate full-duplex exchange");

        assert_eq!(code, 0);
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "relay stalled on full-duplex backpressure: took {:?}",
            started.elapsed()
        );
    }

    #[derive(Default)]
    struct EofBackpressureSession {
        eof_blocked: bool,
        output_drained_after_eof_blocked: bool,
        eof_written: bool,
    }

    impl PtyRelaySession for EofBackpressureSession {
        fn try_write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes == [4, 4] {
                if !self.eof_blocked {
                    self.eof_blocked = true;
                    return Ok(0);
                }
                if !self.output_drained_after_eof_blocked {
                    return Ok(0);
                }
                self.eof_written = true;
            }
            Ok(bytes.len())
        }

        fn eof_bytes(&self) -> std::io::Result<[u8; 2]> {
            Ok([4, 4])
        }

        fn read_available(&mut self) -> std::io::Result<Vec<u8>> {
            if self.eof_blocked {
                self.output_drained_after_eof_blocked = true;
            }
            Ok(Vec::new())
        }

        fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
            Ok(self.eof_written.then(|| ExitStatus::from_raw(0)))
        }
    }

    #[test]
    fn relay_keeps_draining_output_while_eof_is_backpressured() {
        let code = relay_pty_stdio(
            EofBackpressureSession::default(),
            std::io::Cursor::new(b"input"),
            std::io::sink(),
        )
        .expect("relay must route EOF through the nonblocking input pump");

        assert_eq!(code, 0);
    }

    #[test]
    fn preserves_normal_exit_code() {
        // Raw status with exit code 3 lives in bits 8..15.
        assert_eq!(exit_code_from_status(ExitStatus::from_raw(3 << 8)), 3);
        assert_eq!(exit_code_from_status(ExitStatus::from_raw(0)), 0);
    }

    #[test]
    fn maps_signal_termination_to_128_plus_signal() {
        // Raw status where the low 7 bits hold the terminating signal.
        assert_eq!(exit_code_from_status(ExitStatus::from_raw(15)), 143); // SIGTERM
        assert_eq!(exit_code_from_status(ExitStatus::from_raw(9)), 137); // SIGKILL
    }
}
