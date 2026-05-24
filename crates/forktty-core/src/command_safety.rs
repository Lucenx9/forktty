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

/// Returns true when `program` + `first_arg` look like `sh -c <something>`.
///
/// Accepted shell basenames cover the common POSIX shells plus anything ending
/// in `sh` (so `tcsh`, `csh`, `xonsh`, etc. are caught too).
pub fn is_shell_trampoline(program: &str, first_arg: Option<&str>) -> bool {
    if first_arg != Some("-c") {
        return false;
    }
    let shell = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    matches!(shell, "sh" | "bash" | "dash" | "zsh" | "fish" | "ksh") || shell.ends_with("sh")
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
/// Rejects empty strings, names longer than 255 bytes, embedded NULs and
/// backslashes, and any path segment that is empty, `.`, or `..`. The trimmed
/// slice is borrowed from `name`, so callers can keep the original allocation.
pub fn validate_worktree_name(name: &str) -> Result<&str, WorktreeNameError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(WorktreeNameError::Empty);
    }
    if trimmed.len() > 255 {
        return Err(WorktreeNameError::TooLong);
    }
    if trimmed.contains('\0') || trimmed.contains('\\') {
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
        assert!(is_shell_trampoline("/bin/sh", Some("-c")));
        assert!(is_shell_trampoline("/usr/bin/bash", Some("-c")));
        assert!(is_shell_trampoline("/usr/bin/zsh", Some("-c")));
        assert!(is_shell_trampoline("/opt/homebrew/bin/tcsh", Some("-c")));
    }

    #[test]
    fn shell_trampoline_ignores_non_shell_programs() {
        assert!(!is_shell_trampoline("/bin/sh", None));
        assert!(!is_shell_trampoline("/bin/sh", Some("script.sh")));
        assert!(!is_shell_trampoline("/usr/bin/notify-send", Some("-c")));
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
            validate_worktree_name("../escape"),
            Err(WorktreeNameError::UnsafeSegment)
        );
        assert_eq!(
            validate_worktree_name("feature//empty"),
            Err(WorktreeNameError::UnsafeSegment)
        );
        assert_eq!(
            validate_worktree_name("feature\\windows"),
            Err(WorktreeNameError::UnsupportedCharacters)
        );
        assert_eq!(
            validate_worktree_name(&"x".repeat(256)),
            Err(WorktreeNameError::TooLong)
        );
    }
}
