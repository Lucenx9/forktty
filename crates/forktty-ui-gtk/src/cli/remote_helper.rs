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
    use std::io::Read;

    let cwd = std::env::current_dir()?;
    let request = remote_helper_pty_request(argv, cwd)?;
    let mut session = PtySession::spawn(&request, PtySize { cols: 80, rows: 24 })?;
    // Keep the stdin relay bounded so a child that stops draining its PTY
    // applies backpressure to the helper instead of letting queued input grow
    // without limit.
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(16);
    std::thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        let mut buf = [0u8; 8192];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) if tx.send(buf[..n].to_vec()).is_err() => break,
                Ok(_) => {}
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    });
    let mut stdout = io::stdout().lock();
    loop {
        for bytes in rx.try_iter() {
            session.write_all(&bytes)?;
        }
        write_pty_output(&mut session, &mut stdout)?;
        if let Some(status) = session.try_wait()? {
            write_pty_output(&mut session, &mut stdout)?;
            return Ok(status.code().unwrap_or(1));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(feature = "gtk-ghostty")]
fn write_pty_output<W: std::io::Write>(
    session: &mut forktty_terminal::ghostty::pty::PtySession,
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
