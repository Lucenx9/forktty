//! Panic hook that appends panic reports to a file under the state dir.
//!
//! Panics inside GTK signal trampolines abort the process, so the only
//! post-mortem artifact is a coredump that needs manual symbolization.
//! Logging message + location + backtrace to a file makes field crashes
//! diagnosable immediately. The previous hook is chained, so stderr
//! output and `RUST_BACKTRACE` behavior are unchanged.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const PANIC_LOG_FILE: &str = "panic.log";

pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(path) = panic_log_path() {
            let entry = format_panic_entry(
                info.payload_as_str()
                    .unwrap_or("<non-string panic payload>"),
                info.location()
                    .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                    .as_deref(),
                std::thread::current().name().unwrap_or("<unnamed>"),
                &std::backtrace::Backtrace::force_capture().to_string(),
                &unix_timestamp(),
                env!("CARGO_PKG_VERSION"),
            );
            append_to_log(&path, &entry);
        }
        previous(info);
    }));
}

/// Mirrors the state-dir resolution in `forktty_core::session` and `cli.rs`.
fn panic_log_path() -> Option<PathBuf> {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|d| d.join("forktty").join(PANIC_LOG_FILE))
}

fn unix_timestamp() -> String {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => format!("@{}", d.as_secs()),
        Err(_) => "@unknown".to_string(),
    }
}

fn format_panic_entry(
    message: &str,
    location: Option<&str>,
    thread: &str,
    backtrace: &str,
    timestamp: &str,
    version: &str,
) -> String {
    format!(
        "==== panic {timestamp} (forktty {version}) ====\n\
         thread: {thread}\n\
         message: {message}\n\
         location: {}\n\
         backtrace:\n{backtrace}\n",
        location.unwrap_or("<unknown>"),
    )
}

/// Best-effort append; a panic hook must never panic (that would abort
/// before the chained hook runs), so every failure here is swallowed.
fn append_to_log(path: &Path, entry: &str) {
    if let Some(parent) = path.parent() {
        let _ = fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent);
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
    }
    if !prepare_private_log_path(path) {
        return;
    }
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
    {
        let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
        let _ = file.write_all(entry.as_bytes());
    }
}

fn prepare_private_log_path(path: &Path) -> bool {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => return error.kind() == std::io::ErrorKind::NotFound,
    };
    if metadata.file_type().is_file() && metadata.permissions().mode() & 0o077 == 0 {
        return true;
    }

    fs::rename(path, insecure_log_backup_path(path)).is_ok()
}

fn insecure_log_backup_path(path: &Path) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut backup = path.as_os_str().to_owned();
    backup.push(format!(".insecure-{}-{nanos}", std::process::id()));
    PathBuf::from(backup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_includes_message_location_and_metadata() {
        let entry = format_panic_entry(
            "index out of bounds",
            Some("src/gtk_app/terminal_widget.rs:761:13"),
            "main",
            "0: rust_begin_unwind",
            "@1765000000",
            "0.2.0",
        );
        assert!(entry.contains("message: index out of bounds"));
        assert!(entry.contains("location: src/gtk_app/terminal_widget.rs:761:13"));
        assert!(entry.contains("thread: main"));
        assert!(entry.contains("forktty 0.2.0"));
        assert!(entry.contains("rust_begin_unwind"));
    }

    #[test]
    fn missing_location_is_marked_unknown() {
        let entry = format_panic_entry("boom", None, "main", "", "@0", "0.2.0");
        assert!(entry.contains("location: <unknown>"));
    }

    #[test]
    fn append_creates_private_parent_dirs_and_appends_to_private_log() {
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = dir.path().join("nested");
        let path = parent.join("panic.log");
        append_to_log(&path, "first\n");
        append_to_log(&path, "second\n");
        let contents = std::fs::read_to_string(&path).expect("log file");
        assert_eq!(contents, "first\nsecond\n");
        assert_eq!(
            std::fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn append_replaces_existing_world_readable_log_before_writing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("panic.log");
        std::fs::write(&path, "old\n").expect("write old log");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("chmod old log");

        append_to_log(&path, "new\n");

        assert_eq!(std::fs::read_to_string(&path).expect("new log"), "new\n");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let backups = std::fs::read_dir(dir.path())
            .expect("read temp dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("panic.log.insecure-")
            })
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1);
        assert_eq!(
            std::fs::read_to_string(backups[0].path()).expect("old log backup"),
            "old\n"
        );
    }
}
