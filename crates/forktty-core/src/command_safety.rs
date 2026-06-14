//! Shared validators for user-supplied commands and worktree names.
//!
//! These checks are intentionally conservative: ForkTTY runs notification
//! commands and worktree hooks with the user's full privileges, and we want a
//! single, consistent definition of "this looks like a path to an executable
//! file, and not a shell trampoline" so that the socket layer, GTK shell, and
//! notification dispatcher cannot drift apart.

use std::fs;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Returns true when `program` + `args` look like `sh -c <something>`.
///
/// Accepted shell basenames cover the common POSIX shells plus anything ending
/// in `sh` (so `tcsh`, `csh`, `xonsh`, etc. are caught too). `-c` is detected
/// anywhere in the leading flag arguments, including clustered short options
/// (`-lc`, `-xc`); scanning stops at the first non-flag argument or at `--`.
/// A leading `env` (with its flags and `VAR=val` assignments) is unwrapped so
/// `env bash -c <something>` is caught too.
pub fn is_shell_trampoline<S: AsRef<str>>(program: &str, args: &[S]) -> bool {
    let basename = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if basename == "env" {
        return env_invokes_shell_trampoline(args);
    }
    let is_shell = matches!(basename, "sh" | "bash" | "dash" | "zsh" | "fish" | "ksh")
        || basename.ends_with("sh");
    if !is_shell {
        return false;
    }
    for arg in args {
        let arg = arg.as_ref();
        if arg == "--" || !arg.starts_with('-') {
            // End of flags: the next token is a script path, not a command
            // string.
            return false;
        }
        if arg == "-c" || (!arg.starts_with("--") && arg.contains('c')) {
            return true;
        }
    }
    false
}

fn env_invokes_shell_trampoline<S: AsRef<str>>(args: &[S]) -> bool {
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_ref();
        if arg == "--" {
            return args
                .get(index + 1)
                .is_some_and(|program| is_shell_trampoline(program.as_ref(), &args[index + 2..]));
        }
        if matches!(arg, "-S" | "--split-string") {
            return args.get(index + 1).is_some_and(|value| {
                split_env_string_invokes_shell(value.as_ref(), &args[index + 2..])
            });
        }
        if let Some(value) = arg.strip_prefix("--split-string=") {
            return split_env_string_invokes_shell(value, &args[index + 1..]);
        }
        if matches!(arg, "-u" | "--unset" | "-C" | "--chdir") {
            index += 2;
            continue;
        }
        if arg.starts_with("--unset=") || arg.starts_with("--chdir=") {
            index += 1;
            continue;
        }
        if arg.starts_with('-') || arg.contains('=') {
            index += 1;
            continue;
        }
        return is_shell_trampoline(arg, &args[index + 1..]);
    }
    false
}

fn split_env_string_invokes_shell<S: AsRef<str>>(value: &str, tail: &[S]) -> bool {
    let Ok(parts) = shell_words::split(value) else {
        return false;
    };
    let Some((program, split_args)) = parts.split_first() else {
        return false;
    };
    let args = split_args
        .iter()
        .map(String::as_str)
        .chain(tail.iter().map(AsRef::as_ref))
        .collect::<Vec<_>>();
    is_shell_trampoline(program, &args)
}

/// Returns true when `path` is an absolute path to a regular executable file.
///
/// On non-Unix targets the executable-bit check is skipped; the path still has
/// to exist and be a regular file.
pub fn is_executable_file(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Returns `true` when `host` is an acceptable SSH target string.
///
/// Conservative but practical: allows normal `user@host`, bare hostnames,
/// IPv6 literals `[::1]`, SSH config aliases (alphanumerics, `.`, `-`, `_`,
/// `@`, `:`, `[`, `]`).  Rejects: empty strings, whitespace, control
/// characters, and values starting with `-` (flag-injection guard).
pub fn is_valid_ssh_host(host: &str) -> bool {
    if host.is_empty() {
        return false;
    }
    // Guard: a leading `-` would be parsed as an ssh flag.
    if host.starts_with('-') {
        return false;
    }
    // Reject any whitespace or control characters.
    if host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    // Allow only characters that legitimately appear in an ssh target:
    // alphanumerics, `.`, `-`, `_`, `@`, `:`, `[`, `]`, `%` (zone IDs).
    host.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@' | ':' | '[' | ']' | '%')
    })
}

/// Returns a trimmed, validated worktree/branch name suitable for passing to
/// `git worktree add` and to filesystem APIs.
///
/// Rejects empty strings, names longer than 255 bytes, leading dashes, control
/// characters, backslashes, and any path segment that is empty, `.`, or `..`.
/// The trimmed slice is borrowed from `name`, so callers can keep the original
/// allocation.
pub fn validate_worktree_name(name: &str) -> Result<&str, WorktreeNameError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(WorktreeNameError::Empty);
    }
    if trimmed.len() > 255 {
        return Err(WorktreeNameError::TooLong);
    }
    if trimmed.starts_with('-') || trimmed.contains('\\') || trimmed.chars().any(char::is_control) {
        return Err(WorktreeNameError::UnsupportedCharacters);
    }
    if trimmed
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(WorktreeNameError::UnsafeSegment);
    }
    Ok(trimmed)
}

/// Reasons `validate_worktree_name` rejects an input. Callers can map these to
/// their own user-facing wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeNameError {
    Empty,
    TooLong,
    UnsupportedCharacters,
    UnsafeSegment,
}

impl std::fmt::Display for WorktreeNameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorktreeNameError::Empty => f.write_str("must not be empty"),
            WorktreeNameError::TooLong => f.write_str("must be 255 bytes or fewer"),
            WorktreeNameError::UnsupportedCharacters => {
                f.write_str("contains unsupported characters")
            }
            WorktreeNameError::UnsafeSegment => f.write_str("contains an unsafe path segment"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_trampoline_detects_common_shells() {
        assert!(is_shell_trampoline("/bin/sh", &["-c"]));
        assert!(is_shell_trampoline("/usr/bin/bash", &["-c"]));
        assert!(is_shell_trampoline("/usr/bin/zsh", &["-c"]));
        assert!(is_shell_trampoline("/opt/homebrew/bin/tcsh", &["-c"]));
    }

    #[test]
    fn shell_trampoline_detects_clustered_and_separated_flags() {
        assert!(is_shell_trampoline("/bin/bash", &["-lc", "echo hi"]));
        assert!(is_shell_trampoline("/bin/bash", &["-x", "-c", "echo hi"]));
        assert!(is_shell_trampoline("/usr/bin/zsh", &["-ic", "echo hi"]));
    }

    #[test]
    fn shell_trampoline_unwraps_leading_env() {
        assert!(is_shell_trampoline(
            "/usr/bin/env",
            &["bash", "-c", "echo hi"]
        ));
        assert!(is_shell_trampoline(
            "/usr/bin/env",
            &["-i", "FOO=bar", "sh", "-lc", "echo hi"]
        ));
        assert!(!is_shell_trampoline(
            "/usr/bin/env",
            &["notify-send", "-c", "x"]
        ));
        assert!(!is_shell_trampoline("/usr/bin/env", &["FOO=bar"]));
    }

    #[test]
    fn shell_trampoline_unwraps_env_options_with_values() {
        assert!(is_shell_trampoline(
            "/usr/bin/env",
            &["-u", "PATH", "sh", "-c", "echo hi"]
        ));
        assert!(is_shell_trampoline(
            "/usr/bin/env",
            &["--unset=PATH", "bash", "-lc", "echo hi"]
        ));
        assert!(is_shell_trampoline(
            "/usr/bin/env",
            &[
                "--ignore-environment",
                "--unset",
                "HOME",
                "zsh",
                "-ic",
                "echo hi"
            ]
        ));
    }

    #[test]
    fn shell_trampoline_unwraps_env_split_string() {
        assert!(is_shell_trampoline(
            "/usr/bin/env",
            &["-S", "sh -c 'echo hi'"]
        ));
        assert!(is_shell_trampoline(
            "/usr/bin/env",
            &["-S", "sh", "-c", "echo hi"]
        ));
        assert!(is_shell_trampoline(
            "/usr/bin/env",
            &["--split-string=bash -lc 'echo hi'"]
        ));
        assert!(is_shell_trampoline(
            "/usr/bin/env",
            &["--split-string=sh", "-c", "echo hi"]
        ));
        assert!(!is_shell_trampoline(
            "/usr/bin/env",
            &["-S", "notify-send -c x"]
        ));
    }

    #[test]
    fn shell_trampoline_ignores_non_shell_programs() {
        let none: &[&str] = &[];
        assert!(!is_shell_trampoline("/bin/sh", none));
        assert!(!is_shell_trampoline("/bin/sh", &["script.sh"]));
        assert!(!is_shell_trampoline("/bin/sh", &["script.sh", "-c"]));
        assert!(!is_shell_trampoline("/bin/sh", &["--", "-c"]));
        assert!(!is_shell_trampoline("/bin/bash", &["-l"]));
        assert!(!is_shell_trampoline("/usr/bin/notify-send", &["-c"]));
    }

    #[test]
    fn is_executable_file_requires_absolute_existing_file() {
        assert!(!is_executable_file(Path::new("relative/path")));
        assert!(!is_executable_file(Path::new("/definitely/missing/path")));
        // /bin/sh exists and is executable on every supported platform.
        assert!(is_executable_file(Path::new("/bin/sh")));
    }

    #[test]
    fn validate_worktree_name_accepts_and_trims() {
        assert_eq!(validate_worktree_name(" feature/x ").unwrap(), "feature/x");
        assert_eq!(
            validate_worktree_name(&"x".repeat(255)).unwrap(),
            "x".repeat(255)
        );
    }

    #[test]
    fn is_valid_ssh_host_accepts_well_formed_targets() {
        assert!(is_valid_ssh_host("user@host"));
        assert!(is_valid_ssh_host("example.com"));
        assert!(is_valid_ssh_host("host.example.com"));
        assert!(is_valid_ssh_host("[::1]"));
        assert!(is_valid_ssh_host("user@host.example.com"));
        assert!(is_valid_ssh_host("my-server"));
        assert!(is_valid_ssh_host("my_alias"));
        assert!(is_valid_ssh_host("[fe80::1%eth0]"));
    }

    #[test]
    fn is_valid_ssh_host_rejects_invalid_targets() {
        // Empty
        assert!(!is_valid_ssh_host(""));
        // Leading dash (flag injection)
        assert!(!is_valid_ssh_host("-oProxyCommand=x"));
        assert!(!is_valid_ssh_host("-l user"));
        // Whitespace
        assert!(!is_valid_ssh_host("a b"));
        assert!(!is_valid_ssh_host("host\targ"));
        // Newline
        assert!(!is_valid_ssh_host("host\narg"));
        // Control character
        assert!(!is_valid_ssh_host("host\x00"));
    }

    #[test]
    fn validate_worktree_name_rejects_unsafe_inputs() {
        assert_eq!(validate_worktree_name(""), Err(WorktreeNameError::Empty));
        assert_eq!(
            validate_worktree_name("   \t  "),
            Err(WorktreeNameError::Empty)
        );
        assert_eq!(
            validate_worktree_name("../escape"),
            Err(WorktreeNameError::UnsafeSegment)
        );
        assert_eq!(
            validate_worktree_name("feature//empty"),
            Err(WorktreeNameError::UnsafeSegment)
        );
        assert_eq!(
            validate_worktree_name("/feature"),
            Err(WorktreeNameError::UnsafeSegment)
        );
        assert_eq!(
            validate_worktree_name("feature/"),
            Err(WorktreeNameError::UnsafeSegment)
        );
        assert_eq!(
            validate_worktree_name("feature/."),
            Err(WorktreeNameError::UnsafeSegment)
        );
        assert_eq!(
            validate_worktree_name("."),
            Err(WorktreeNameError::UnsafeSegment)
        );
        assert_eq!(
            validate_worktree_name("feature\\windows"),
            Err(WorktreeNameError::UnsupportedCharacters)
        );
        assert_eq!(
            validate_worktree_name("-flag"),
            Err(WorktreeNameError::UnsupportedCharacters)
        );
        assert_eq!(
            validate_worktree_name("feature\nname"),
            Err(WorktreeNameError::UnsupportedCharacters)
        );
        assert_eq!(
            validate_worktree_name("feature\0name"),
            Err(WorktreeNameError::UnsupportedCharacters)
        );
        assert_eq!(
            validate_worktree_name(&"x".repeat(256)),
            Err(WorktreeNameError::TooLong)
        );
    }
}
