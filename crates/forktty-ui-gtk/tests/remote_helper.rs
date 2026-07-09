#![cfg(feature = "gtk-ghostty")]

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn remote_helper_pty_propagates_unterminated_stdin_eof() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_forktty"))
        .args(["remote-helper", "pty", "--", "/bin/cat"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"hello").unwrap();
    drop(stdin);

    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("remote helper did not propagate stdin EOF within 5 seconds");
        }
        std::thread::sleep(Duration::from_millis(10));
    };

    let mut stdout = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut stdout)
        .unwrap();
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();

    assert!(
        status.success(),
        "remote helper exited with {status}: {stderr}"
    );
    assert_eq!(stdout, b"hellohello");
}
