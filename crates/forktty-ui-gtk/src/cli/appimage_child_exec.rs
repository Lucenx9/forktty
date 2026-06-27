//! Internal AppImage child-process launcher.
//!
//! Ghostty-spawned terminal children can otherwise inherit AppImage runtime file
//! descriptors from the GTK process. This hidden helper closes inherited
//! descriptors before replacing itself with the actual terminal command.

use forktty_terminal::spawn::APPIMAGE_CHILD_EXEC_SUBCOMMAND;
use std::ffi::{OsStr, OsString};

const USAGE: &str = "appimage-child-exec requires -- <program> [args...]";

pub(super) fn parse_appimage_child_exec_args(args: &[OsString]) -> Result<Vec<OsString>, String> {
    if args.first().map(OsString::as_os_str) != Some(OsStr::new("--")) {
        return Err(USAGE.to_string());
    }
    let argv = args[1..].to_vec();
    if argv.is_empty() {
        return Err(USAGE.to_string());
    }
    Ok(argv)
}

#[cfg(unix)]
pub fn run_appimage_child_exec(argv: Vec<OsString>) -> i32 {
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    let Some(program) = argv.first() else {
        eprintln!("forktty {APPIMAGE_CHILD_EXEC_SUBCOMMAND}: {USAGE}");
        return 2;
    };

    close_inherited_fds();
    let err = Command::new(program).args(&argv[1..]).exec();
    eprintln!(
        "forktty {APPIMAGE_CHILD_EXEC_SUBCOMMAND}: failed to exec {}: {err}",
        program.to_string_lossy()
    );
    exec_error_status(&err)
}

#[cfg(not(unix))]
pub fn run_appimage_child_exec(_argv: Vec<OsString>) -> i32 {
    eprintln!("forktty {APPIMAGE_CHILD_EXEC_SUBCOMMAND}: unsupported platform");
    1
}

#[cfg(unix)]
fn close_inherited_fds() {
    let fds = fd_numbers_from_proc_self().unwrap_or_else(fallback_fd_numbers);
    for fd in fds {
        if fd > 2 {
            // SAFETY: closing an owned or inherited raw file descriptor is the
            // purpose of this helper. EBADF is harmless because descriptors may
            // have already been closed while collecting `/proc/self/fd`.
            unsafe {
                libc::close(fd);
            }
        }
    }
}

#[cfg(unix)]
fn fd_numbers_from_proc_self() -> Option<Vec<libc::c_int>> {
    let entries = std::fs::read_dir("/proc/self/fd").ok()?;
    let mut fds = entries
        .filter_map(Result::ok)
        .filter_map(|entry| parse_fd_number(&entry.file_name()))
        .filter(|fd| *fd > 2)
        .collect::<Vec<_>>();
    fds.sort_unstable();
    fds.dedup();
    Some(fds)
}

#[cfg(unix)]
fn parse_fd_number(name: &OsStr) -> Option<libc::c_int> {
    name.to_str()?.parse::<libc::c_int>().ok()
}

#[cfg(unix)]
fn fallback_fd_numbers() -> Vec<libc::c_int> {
    (3..1024).collect()
}

#[cfg(unix)]
fn exec_error_status(err: &std::io::Error) -> i32 {
    match err.kind() {
        std::io::ErrorKind::NotFound => 127,
        std::io::ErrorKind::PermissionDenied => 126,
        _ => 126,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_fd_number_accepts_numeric_fd_names() {
        assert_eq!(parse_fd_number(OsStr::new("1023")), Some(1023));
        assert_eq!(parse_fd_number(OsStr::new("not-a-fd")), None);
    }

    #[test]
    fn parse_appimage_child_exec_args_rejects_invalid_forms() {
        assert_eq!(
            parse_appimage_child_exec_args(&[OsString::from("/usr/bin/dtach")]).unwrap_err(),
            USAGE.to_string()
        );
        assert_eq!(
            parse_appimage_child_exec_args(&[OsString::from("--")]).unwrap_err(),
            USAGE.to_string()
        );
    }
}
