//! Internal AppImage child-process launcher.
//!
//! Ghostty-spawned terminal children can otherwise inherit AppImage runtime file
//! descriptors from the GTK process. This hidden helper closes inherited
//! descriptors before replacing itself with the actual terminal command.

use forktty_terminal::spawn::APPIMAGE_CHILD_EXEC_SUBCOMMAND;
use std::ffi::{OsStr, OsString};
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::process::Command;

const USAGE: &str = "appimage-child-exec requires -- <program> [args...]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppImageChildExecRequest {
    pub env: Vec<(OsString, OsString)>,
    pub unset: Vec<OsString>,
    pub argv: Vec<OsString>,
}

pub(super) fn parse_appimage_child_exec_args(
    args: &[OsString],
) -> Result<AppImageChildExecRequest, String> {
    let mut env = Vec::new();
    let mut unset = Vec::new();
    let mut index = 0;
    loop {
        match args.get(index).map(OsString::as_os_str) {
            Some(option) if option == OsStr::new("--env") => {
                let Some(raw) = args.get(index + 1) else {
                    return Err(USAGE.to_string());
                };
                env.push(parse_env_assignment(raw)?);
                index += 2;
            }
            Some(option) if option == OsStr::new("--unset") => {
                let Some(raw) = args.get(index + 1) else {
                    return Err(USAGE.to_string());
                };
                unset.push(parse_unset_key(raw)?);
                index += 2;
            }
            Some(separator) if separator == OsStr::new("--") => break,
            _ => return Err(USAGE.to_string()),
        }
    }
    let argv = args[index + 1..].to_vec();
    if argv.is_empty() {
        return Err(USAGE.to_string());
    }
    Ok(AppImageChildExecRequest { env, unset, argv })
}

fn parse_env_assignment(raw: &OsStr) -> Result<(OsString, OsString), String> {
    parse_env_assignment_for_platform(raw)
}

fn parse_unset_key(raw: &OsStr) -> Result<OsString, String> {
    validate_env_key(raw)?;
    Ok(raw.to_os_string())
}

#[cfg(unix)]
fn parse_env_assignment_for_platform(raw: &OsStr) -> Result<(OsString, OsString), String> {
    let bytes = raw.as_bytes();
    let Some(separator) = bytes.iter().position(|byte| *byte == b'=') else {
        return Err(USAGE.to_string());
    };
    let key = OsString::from_vec(bytes[..separator].to_vec());
    let value = OsString::from_vec(bytes[separator + 1..].to_vec());
    validate_env_key(&key)?;
    validate_env_value(&value)?;
    Ok((key, value))
}

#[cfg(not(unix))]
fn parse_env_assignment_for_platform(raw: &OsStr) -> Result<(OsString, OsString), String> {
    let assignment = raw.to_string_lossy();
    let Some((key, value)) = assignment.split_once('=') else {
        return Err(USAGE.to_string());
    };
    let key = OsString::from(key);
    let value = OsString::from(value);
    validate_env_key(&key)?;
    validate_env_value(&value)?;
    Ok((key, value))
}

#[cfg(unix)]
fn validate_env_key(key: &OsStr) -> Result<(), String> {
    let bytes = key.as_bytes();
    if bytes.is_empty() || bytes.contains(&b'=') || bytes.contains(&0) {
        return Err(USAGE.to_string());
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_env_key(key: &OsStr) -> Result<(), String> {
    let key = key.to_string_lossy();
    if key.is_empty() || key.contains('=') || key.contains('\0') {
        return Err(USAGE.to_string());
    }
    Ok(())
}

#[cfg(unix)]
fn validate_env_value(value: &OsStr) -> Result<(), String> {
    if value.as_bytes().contains(&0) {
        return Err(USAGE.to_string());
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_env_value(value: &OsStr) -> Result<(), String> {
    if value.to_string_lossy().contains('\0') {
        return Err(USAGE.to_string());
    }
    Ok(())
}

fn appimage_child_command(request: &AppImageChildExecRequest) -> Result<Command, String> {
    let Some(program) = request.argv.first() else {
        return Err(USAGE.to_string());
    };
    for key in &request.unset {
        validate_env_key(key)?;
    }
    for (key, value) in &request.env {
        validate_env_key(key)?;
        validate_env_value(value)?;
    }

    let mut command = Command::new(program);
    command.args(&request.argv[1..]);
    for key in &request.unset {
        command.env_remove(key);
    }
    for (key, value) in &request.env {
        command.env(key, value);
    }
    Ok(command)
}

#[cfg(unix)]
pub fn run_appimage_child_exec(request: AppImageChildExecRequest) -> i32 {
    use std::os::unix::process::CommandExt;

    let Some(program) = request.argv.first().cloned() else {
        eprintln!("forktty {APPIMAGE_CHILD_EXEC_SUBCOMMAND}: {USAGE}");
        return 2;
    };
    let mut command = match appimage_child_command(&request) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("forktty {APPIMAGE_CHILD_EXEC_SUBCOMMAND}: {message}");
            return 2;
        }
    };

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

        #[cfg(unix)]
        for assignment in [b"BAD\0KEY=value".to_vec(), b"KEY=bad\0value".to_vec()] {
            assert_eq!(
                parse_appimage_child_exec_args(&[
                    OsString::from("--env"),
                    OsString::from_vec(assignment),
                    OsString::from("--"),
                    OsString::from("/usr/bin/env"),
                ])
                .unwrap_err(),
                USAGE.to_string()
            );
        }
    }

    #[test]
    fn appimage_child_exec_parses_interleaved_set_and_unset() {
        let request = parse_appimage_child_exec_args(&[
            OsString::from("--env"),
            OsString::from("KEEP=first"),
            OsString::from("--unset"),
            OsString::from("REMOVE"),
            OsString::from("--env"),
            OsString::from("KEEP=second"),
            OsString::from("--unset"),
            OsString::from("ALSO_REMOVE"),
            OsString::from("--"),
            OsString::from("/usr/bin/env"),
        ])
        .unwrap();

        assert_eq!(
            request.env,
            vec![
                (OsString::from("KEEP"), OsString::from("first")),
                (OsString::from("KEEP"), OsString::from("second")),
            ]
        );
        assert_eq!(
            request.unset,
            vec![OsString::from("REMOVE"), OsString::from("ALSO_REMOVE")]
        );
        assert_eq!(request.argv, vec![OsString::from("/usr/bin/env")]);
    }

    #[test]
    fn appimage_child_exec_rejects_invalid_unset_key() {
        for key in [OsString::from(""), OsString::from("BAD=KEY")] {
            assert_eq!(
                parse_appimage_child_exec_args(&[
                    OsString::from("--unset"),
                    key,
                    OsString::from("--"),
                    OsString::from("/usr/bin/env"),
                ])
                .unwrap_err(),
                USAGE.to_string()
            );
        }

        #[cfg(unix)]
        assert_eq!(
            parse_appimage_child_exec_args(&[
                OsString::from("--unset"),
                OsString::from_vec(b"BAD\0KEY".to_vec()),
                OsString::from("--"),
                OsString::from("/usr/bin/env"),
            ])
            .unwrap_err(),
            USAGE.to_string()
        );
    }

    #[test]
    fn appimage_child_exec_preserves_assignment_values_with_equals() {
        let request = parse_appimage_child_exec_args(&[
            OsString::from("--env"),
            OsString::from("QUERY=one=two=three"),
            OsString::from("--"),
            OsString::from("/usr/bin/env"),
        ])
        .unwrap();

        assert_eq!(
            request.env,
            vec![(OsString::from("QUERY"), OsString::from("one=two=three"))]
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
                unset: Vec::new(),
                argv: vec![OsString::from("/usr/bin/dtach"), OsString::from("-A")]
            }
        );
    }

    #[test]
    #[cfg(unix)]
    fn appimage_child_command_applies_environment_delta() {
        const REMOVED_KEY: &str = "FORKTTY_TEST_APPIMAGE_CHILD_REMOVE";
        const OVERRIDDEN_KEY: &str = "FORKTTY_TEST_APPIMAGE_CHILD_OVERRIDE";
        const ASSIGNED_KEY: &str = "FORKTTY_TEST_APPIMAGE_CHILD_ASSIGN";

        crate::test_env::with_env(
            &[
                (REMOVED_KEY, Some("parent-remove")),
                (OVERRIDDEN_KEY, Some("parent-override")),
                (ASSIGNED_KEY, None),
            ],
            || {
                let removed_key = OsString::from(REMOVED_KEY);
                let overridden_key = OsString::from(OVERRIDDEN_KEY);
                let assigned_key = OsString::from(ASSIGNED_KEY);
                let parent_values = [
                    std::env::var_os(&removed_key),
                    std::env::var_os(&overridden_key),
                    std::env::var_os(&assigned_key),
                ];
                let raw_program = OsString::from_vec(b"/tmp/raw-program-\xfe".to_vec());
                let raw_arg = OsString::from_vec(b"raw-\xff=value".to_vec());
                let request = AppImageChildExecRequest {
                    env: vec![
                        (overridden_key.clone(), OsString::from("replacement")),
                        (assigned_key.clone(), OsString::from("assigned=value")),
                    ],
                    unset: vec![removed_key.clone(), overridden_key.clone()],
                    argv: vec![
                        raw_program.clone(),
                        OsString::from("--null"),
                        raw_arg.clone(),
                    ],
                };

                let command = appimage_child_command(&request).unwrap();

                assert_eq!(command.get_program(), raw_program.as_os_str());
                assert_eq!(
                    command.get_args().collect::<Vec<_>>(),
                    vec![OsStr::new("--null"), raw_arg.as_os_str()]
                );
                assert!(command
                    .get_envs()
                    .any(|(key, value)| key == removed_key && value.is_none()));
                assert!(command.get_envs().any(|(key, value)| {
                    key == overridden_key && value == Some(OsStr::new("replacement"))
                }));
                assert!(command.get_envs().any(|(key, value)| {
                    key == assigned_key && value == Some(OsStr::new("assigned=value"))
                }));
                assert_eq!(
                    [
                        std::env::var_os(&removed_key),
                        std::env::var_os(&overridden_key),
                        std::env::var_os(&assigned_key),
                    ],
                    parent_values
                );
            },
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
