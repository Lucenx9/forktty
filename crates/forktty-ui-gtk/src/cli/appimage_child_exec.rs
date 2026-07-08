//! Internal AppImage child-process launcher.
//!
//! Ghostty-spawned terminal children can otherwise inherit AppImage runtime file
//! descriptors from the GTK process. This hidden helper closes inherited
//! descriptors before replacing itself with the actual terminal command.

use forktty_terminal::spawn::APPIMAGE_CHILD_EXEC_SUBCOMMAND;
use std::ffi::{OsStr, OsString};
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};

const USAGE: &str = "appimage-child-exec requires -- <program> [args...]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppImageChildExecRequest {
    pub env: Vec<(OsString, OsString)>,
    pub argv: Vec<OsString>,
}

pub(super) fn parse_appimage_child_exec_args(
    args: &[OsString],
) -> Result<AppImageChildExecRequest, String> {
    let mut env = Vec::new();
    let mut index = 0;
    while args.get(index).map(OsString::as_os_str) == Some(OsStr::new("--env")) {
        let Some(raw) = args.get(index + 1) else {
            return Err(USAGE.to_string());
        };
        env.push(parse_env_assignment(raw)?);
        index += 2;
    }
    if args.get(index).map(OsString::as_os_str) != Some(OsStr::new("--")) {
        return Err(USAGE.to_string());
    }
    let argv = args[index + 1..].to_vec();
    if argv.is_empty() {
        return Err(USAGE.to_string());
    }
    Ok(AppImageChildExecRequest { env, argv })
}

fn parse_env_assignment(raw: &OsString) -> Result<(OsString, OsString), String> {
    parse_env_assignment_for_platform(raw)
}

#[cfg(unix)]
fn parse_env_assignment_for_platform(raw: &OsString) -> Result<(OsString, OsString), String> {
    let bytes = raw.as_os_str().as_bytes();
    let Some(separator) = bytes.iter().position(|byte| *byte == b'=') else {
        return Err(USAGE.to_string());
    };
    if separator == 0 || bytes.contains(&0) {
        return Err(USAGE.to_string());
    }
    Ok((
        OsString::from_vec(bytes[..separator].to_vec()),
        OsString::from_vec(bytes[separator + 1..].to_vec()),
    ))
}

#[cfg(not(unix))]
fn parse_env_assignment_for_platform(raw: &OsString) -> Result<(OsString, OsString), String> {
    let value = raw.to_string_lossy();
    let Some((key, value)) = value.split_once('=') else {
        return Err(USAGE.to_string());
    };
    if key.is_empty() {
        return Err(USAGE.to_string());
    }
    Ok((OsString::from(key), OsString::from(value)))
}

#[cfg(unix)]
pub fn run_appimage_child_exec(request: AppImageChildExecRequest) -> i32 {
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    let Some(program) = request.argv.first() else {
        eprintln!("forktty {APPIMAGE_CHILD_EXEC_SUBCOMMAND}: {USAGE}");
        return 2;
    };

    let mut command = Command::new(program);
    command.args(&request.argv[1..]);
    for (key, value) in &request.env {
        command.env(key, value);
    }
    close_inherited_fds();
    let err = command.exec();
    eprintln!(
        "forktty {APPIMAGE_CHILD_EXEC_SUBCOMMAND}: failed to exec {}: {err}",
        program.to_string_lossy()
    );
    exec_error_status(&err)
}

#[cfg(not(unix))]
pub fn run_appimage_child_exec(_request: AppImageChildExecRequest) -> i32 {
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
        assert_eq!(
            parse_appimage_child_exec_args(&[
                OsString::from("--env"),
                OsString::from("FORKTTY_WORKSPACE_ID=workspace-1"),
                OsString::from("--")
            ])
            .unwrap_err(),
            USAGE.to_string()
        );
        assert_eq!(
            parse_appimage_child_exec_args(&[OsString::from("--env"), OsString::from("--")])
                .unwrap_err(),
            USAGE.to_string()
        );
        assert_eq!(
            parse_appimage_child_exec_args(&[
                OsString::from("--env"),
                OsString::from("FORKTTY_WORKSPACE_ID"),
                OsString::from("--"),
                OsString::from("/usr/bin/dtach")
            ])
            .unwrap_err(),
            USAGE.to_string()
        );
        assert_eq!(
            parse_appimage_child_exec_args(&[
                OsString::from("--env"),
                OsString::from("=workspace-1"),
                OsString::from("--"),
                OsString::from("/usr/bin/dtach")
            ])
            .unwrap_err(),
            USAGE.to_string()
        );
    }

    #[test]
    fn parse_appimage_child_exec_args_preserves_env_and_argv() {
        assert_eq!(
            parse_appimage_child_exec_args(&[
                OsString::from("--env"),
                OsString::from("FORKTTY_WORKSPACE_ID=workspace-1"),
                OsString::from("--env"),
                OsString::from("FORKTTY_SURFACE_ID=surface-1"),
                OsString::from("--"),
                OsString::from("/usr/bin/dtach"),
                OsString::from("-A")
            ])
            .unwrap(),
            AppImageChildExecRequest {
                env: vec![
                    (
                        OsString::from("FORKTTY_WORKSPACE_ID"),
                        OsString::from("workspace-1")
                    ),
                    (
                        OsString::from("FORKTTY_SURFACE_ID"),
                        OsString::from("surface-1")
                    )
                ],
                argv: vec![OsString::from("/usr/bin/dtach"), OsString::from("-A")]
            }
        );
    }

    #[test]
    #[cfg(unix)]
    fn parse_appimage_child_exec_args_preserves_non_utf8_env_bytes() {
        let request = parse_appimage_child_exec_args(&[
            OsString::from("--env"),
            OsString::from_vec(b"RAW=\xff=value".to_vec()),
            OsString::from("--"),
            OsString::from("/usr/bin/env"),
        ])
        .unwrap();

        assert_eq!(
            request.env,
            vec![(
                OsString::from("RAW"),
                OsString::from_vec(b"\xff=value".to_vec())
            )]
        );
    }
}
