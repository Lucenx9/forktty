//! Live persistence proof: a process started under the detach broker keeps
//! running after the launching command returns. This is the property ForkTTY
//! relies on so a generic terminal's process tree survives a GTK UI restart.
//!
//! The test is environment-gated: it runs only when `dtach` is installed
//! (`detect()` finds it) and skips with a printed note otherwise, so it is safe
//! in CI/dev environments without the broker. It uses dtach's `-n`
//! (create-without-attach) mode rather than the production `-A` so it needs no
//! controlling tty; both create a detached daemon — `-A` additionally attaches
//! a client, which is the GTK surface's job at runtime.

#![cfg(unix)]

use forktty_core::pty_persistence::{
    cleanup_managed_session, detect, ensure_private_session_dir, managed_session_cwds,
    session_socket_path, PtyBroker,
};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn process_alive(pid: i32) -> bool {
    // `kill(pid, 0)` probes existence without sending a signal.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[test]
fn process_under_dtach_survives_launcher_exit() {
    let Some(persistence) = detect() else {
        eprintln!("SKIP: no detach broker (dtach) on PATH; persistence survival not exercised");
        return;
    };
    assert_eq!(persistence.broker, PtyBroker::Dtach);

    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("forktty-pty").join("surface-it.sock");
    let pidfile = dir.path().join("child.pid");
    ensure_private_session_dir(&socket).unwrap();

    // Create a detached session whose child records its pid and then idles. The
    // `dtach -n` invocation daemonizes and returns immediately.
    let status = Command::new(&persistence.broker_path)
        .arg("-n")
        .arg(&socket)
        .arg("/bin/sh")
        .arg("-c")
        .arg(format!(
            "echo $$ > {}; exec sleep 30",
            pidfile.to_str().unwrap()
        ))
        .status()
        .expect("spawn dtach");
    assert!(status.success(), "dtach -n exited with {status:?}");

    // The launcher has returned; the child must still be alive and reachable
    // through its persisted socket.
    let pid = wait_for_pid(&pidfile);
    assert!(
        socket.exists(),
        "broker socket should persist after launcher exit"
    );
    assert!(
        process_alive(pid),
        "child {pid} should survive the launcher returning"
    );

    // Clean up the surviving daemon and its child.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
}

#[test]
fn managed_session_cwds_reports_detached_workload_directory() {
    let Some(persistence) = detect() else {
        eprintln!("SKIP: no detach broker (dtach) on PATH; cwd discovery not exercised");
        return;
    };
    let runtime_dir = tempfile::tempdir().unwrap();
    let launch_dir = tempfile::tempdir().unwrap();
    let live_dir = tempfile::tempdir().unwrap();
    let surface_id = "surface-cwd-it";
    let socket = session_socket_path(runtime_dir.path(), surface_id).unwrap();
    ensure_private_session_dir(&socket).unwrap();

    let status = Command::new(&persistence.broker_path)
        .arg("-n")
        .arg(&socket)
        .arg("-E")
        .arg("-z")
        .arg("/bin/sh")
        .arg("-c")
        .arg(format!("cd {} && exec sleep 30", live_dir.path().display()))
        .current_dir(launch_dir.path())
        .status()
        .expect("spawn detached dtach session");
    assert!(status.success(), "dtach -n exited with {status:?}");

    let deadline = Instant::now() + Duration::from_secs(5);
    let observed = loop {
        if let Some(cwd) = managed_session_cwds(runtime_dir.path())
            .unwrap()
            .remove(surface_id)
        {
            break Some(cwd);
        }
        if Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let cleanup = cleanup_managed_session(runtime_dir.path(), surface_id).unwrap();

    assert_eq!(observed.as_deref(), Some(live_dir.path()));
    assert!(cleanup.processes_signaled > 0);
}

fn wait_for_pid(pidfile: &Path) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(contents) = std::fs::read_to_string(pidfile) {
            if let Ok(pid) = contents.trim().parse::<i32>() {
                return pid;
            }
        }
        assert!(
            Instant::now() < deadline,
            "child never wrote its pid file under the broker"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
