//! Shared file and launcher helpers for agent hook, MCP, and skill installers.

use super::{next_file_nonce, CliError, CliResult};
use serde_json::{json, Value};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub(super) const MAX_HOOK_CONFIG_SIZE_BYTES: u64 = 1024 * 1024;

pub(super) fn stable_hook_launcher_path() -> Option<PathBuf> {
    let current_exe = std::env::current_exe().ok();
    stable_hook_launcher_path_from_env(
        current_exe.as_deref(),
        [
            // Launched directly through the appimage runtime.
            (std::env::var_os("APPIMAGE"), std::env::var_os("APPDIR")),
            // Shells spawned inside the AppImage app: the runtime's own vars
            // are stripped from child environments, but AppRun exports these
            // and puts the mounted usr/bin first in PATH, so a plain `forktty`
            // resolves to the mounted binary whose /tmp/.mount_* path dies
            // with the next remount. Hooks must reference the stable AppImage
            // path instead.
            (
                std::env::var_os("FORKTTY_APPIMAGE"),
                std::env::var_os("FORKTTY_APPIMAGE_DIR"),
            ),
        ],
    )
}

/// The launcher path hooks should invoke: the AppImage file when the running
/// binary is the one mounted from it, otherwise the binary itself.
pub(super) fn stable_hook_launcher_path_from_env(
    current_exe: Option<&Path>,
    appimage_candidates: [(Option<OsString>, Option<OsString>); 2],
) -> Option<PathBuf> {
    for (appimage, appdir) in appimage_candidates {
        if let (Some(appimage), Some(appdir), Some(current_exe)) =
            (appimage, appdir, current_exe.as_ref())
        {
            let appimage = PathBuf::from(appimage);
            let appdir = PathBuf::from(appdir);
            if appimage.is_absolute() && appdir.is_absolute() && current_exe.starts_with(appdir) {
                return Some(appimage);
            }
        }
    }
    current_exe.map(Path::to_path_buf)
}

pub(super) fn read_json_file(path: &Path) -> CliResult<Value> {
    read_json_file_with_limit(path, MAX_HOOK_CONFIG_SIZE_BYTES, "hook config")
}

pub(super) fn read_json_file_with_limit(
    path: &Path,
    max_bytes: u64,
    label: &str,
) -> CliResult<Value> {
    let link_meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(json!({})),
        Err(err) => return Err(err.into()),
    };
    let followed = if link_meta.file_type().is_symlink() {
        match fs::metadata(path) {
            Ok(meta) => meta,
            // Treat a broken symlink the same as a missing file: the
            // subsequent write replaces the dangling link with a real file.
            // Previously this aborted `hooks setup` with a confusing
            // "path is a broken symlink" error.
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                eprintln!(
                    "warning: {} is a broken symlink; replacing with a fresh file",
                    path.display()
                );
                return Ok(json!({}));
            }
            Err(err) => return Err(err.into()),
        }
    } else {
        link_meta
    };
    // Reject non-regular files before open(2): opening a FIFO blocks until a
    // peer shows up, which used to hang `forktty hooks setup` forever.
    if !followed.is_file() {
        return Err(CliError::new("path exists but is not a regular file"));
    }
    let file = File::open(path)?;
    // TOCTOU backstop: re-check the opened file, not just the pre-open stat.
    let stat = file.metadata()?;
    if !stat.is_file() {
        return Err(CliError::new("path exists but is not a regular file"));
    }
    if stat.len() > max_bytes {
        return Err(CliError::new(format!(
            "{label} is too large ({} bytes; max {} bytes)",
            stat.len(),
            max_bytes
        )));
    }
    let mut text = String::new();
    let mut limited = file.take(max_bytes + 1);
    limited.read_to_string(&mut text)?;
    if text.len() as u64 > max_bytes {
        return Err(CliError::new(format!(
            "{label} is too large ({} bytes; max {} bytes)",
            text.len(),
            max_bytes
        )));
    }
    if text.trim().is_empty() {
        Ok(json!({}))
    } else {
        serde_json::from_str(&text).map_err(Into::into)
    }
}

pub(super) fn read_text_config(path: &Path, label: &str) -> CliResult<Option<String>> {
    read_text_config_with_limit(path, label, MAX_HOOK_CONFIG_SIZE_BYTES)
}

pub(super) fn read_text_config_with_limit(
    path: &Path,
    label: &str,
    max_bytes: u64,
) -> CliResult<Option<String>> {
    let link_meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let followed = if link_meta.file_type().is_symlink() {
        match fs::metadata(path) {
            Ok(meta) => meta,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                eprintln!(
                    "warning: {} is a broken symlink; replacing with a fresh file",
                    path.display()
                );
                return Ok(None);
            }
            Err(err) => return Err(err.into()),
        }
    } else {
        link_meta
    };
    if !followed.is_file() {
        return Err(CliError::new(format!(
            "{label} exists but is not a regular file"
        )));
    }
    let file = File::open(path)?;
    let stat = file.metadata()?;
    if !stat.is_file() {
        return Err(CliError::new(format!(
            "{label} exists but is not a regular file"
        )));
    }
    if stat.len() > max_bytes {
        return Err(CliError::new(format!(
            "{label} is too large ({} bytes; max {} bytes)",
            stat.len(),
            max_bytes
        )));
    }
    let mut text = String::new();
    let mut limited = file.take(max_bytes + 1);
    limited.read_to_string(&mut text)?;
    if text.len() as u64 > max_bytes {
        return Err(CliError::new(format!(
            "{label} is too large ({} bytes; max {} bytes)",
            text.len(),
            max_bytes
        )));
    }
    Ok(Some(text))
}

pub(super) fn hook_config_write_path(path: &Path) -> CliResult<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => match fs::canonicalize(path) {
            Ok(resolved) => Ok(resolved),
            // Broken symlink: rename will replace the dangling link with the
            // newly written hook config.
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(path.to_path_buf()),
            Err(err) => Err(err.into()),
        },
        Ok(_) => Ok(path.to_path_buf()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(err) => Err(err.into()),
    }
}

pub(super) fn ensure_parent_dir(path: &Path) -> CliResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

pub(super) fn backup_file(path: &Path) -> CliResult<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    loop {
        let backup = path.with_file_name(format!(
            "{}.bak-{}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("config"),
            next_file_nonce()
        ));
        match copy_file_exclusive(path, &backup) {
            Ok(()) => return Ok(Some(backup)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.into()),
        }
    }
}

fn copy_file_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    let mut src = File::open(from)?;
    let mut dst = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(to)?;
    io::copy(&mut src, &mut dst)?;
    dst.sync_all()
}

pub(super) fn atomic_write_file(path: &Path, content: &[u8]) -> CliResult<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let base = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    let tmp = dir.join(format!(".{base}.tmp-{}", next_file_nonce()));
    let result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(content)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(Into::into)
}
