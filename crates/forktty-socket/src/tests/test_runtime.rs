//! Private runtime directories for Unix-socket tests.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::Path;
use std::rc::Rc;

pub(super) struct PrivateRuntimeTempDir {
    socket_dir: tempfile::TempDir,
    _synthetic_runtime: Option<SyntheticRuntimeEnv>,
}

impl PrivateRuntimeTempDir {
    pub(super) fn path(&self) -> &Path {
        self.socket_dir.path()
    }
}

struct SyntheticRuntimeEnv {
    previous: Option<OsString>,
    _guard: super::EnvTestLockGuard,
    _runtime_dir: tempfile::TempDir,
    _not_send: std::marker::PhantomData<Rc<()>>,
}

impl Drop for SyntheticRuntimeEnv {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => unsafe { std::env::set_var("XDG_RUNTIME_DIR", value) },
            None => unsafe { std::env::remove_var("XDG_RUNTIME_DIR") },
        }
    }
}

pub(super) fn private_runtime_tempdir(prefix: &str) -> PrivateRuntimeTempDir {
    if let Some(runtime_dir) = super::env_var_os("XDG_RUNTIME_DIR") {
        if let Some(socket_dir) = private_tempdir_in(Path::new(&runtime_dir), prefix) {
            return PrivateRuntimeTempDir {
                socket_dir,
                _synthetic_runtime: None,
            };
        }
    }

    let guard = super::lock_env_for_test();
    let previous = std::env::var_os("XDG_RUNTIME_DIR");
    if let Some(runtime_dir) = previous.as_deref().map(Path::new) {
        if let Some(socket_dir) = private_tempdir_in(runtime_dir, prefix) {
            return PrivateRuntimeTempDir {
                socket_dir,
                _synthetic_runtime: None,
            };
        }
    }

    let runtime_dir = private_tempdir("forktty-test-xdg-runtime-")
        .expect("create synthetic XDG runtime directory");
    let synthetic_runtime = SyntheticRuntimeEnv {
        previous,
        _guard: guard,
        _runtime_dir: runtime_dir,
        _not_send: std::marker::PhantomData,
    };
    // SAFETY: test-only mutation serialized with every ForkTTY env reader and
    // retained by `SyntheticRuntimeEnv` until the socket fixture is dropped.
    unsafe { std::env::set_var("XDG_RUNTIME_DIR", synthetic_runtime._runtime_dir.path()) };
    let socket_dir = private_tempdir_in(synthetic_runtime._runtime_dir.path(), prefix)
        .expect("create socket directory under synthetic XDG runtime directory");
    debug_assert!(socket_dir
        .path()
        .starts_with(synthetic_runtime._runtime_dir.path()));
    PrivateRuntimeTempDir {
        socket_dir,
        _synthetic_runtime: Some(synthetic_runtime),
    }
}

fn private_tempdir_in(runtime_dir: &Path, prefix: &str) -> Option<tempfile::TempDir> {
    if !runtime_dir.is_absolute() || !runtime_dir.is_dir() {
        return None;
    }
    let mut builder = tempfile::Builder::new();
    builder
        .prefix(prefix)
        .permissions(fs::Permissions::from_mode(0o700));
    let dir = builder.tempdir_in(runtime_dir).ok()?;
    validate_private_dir(&dir);
    Some(dir)
}

fn private_tempdir(prefix: &str) -> std::io::Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder
        .prefix(prefix)
        .permissions(fs::Permissions::from_mode(0o700));
    let dir = builder.tempdir()?;
    validate_private_dir(&dir);
    Ok(dir)
}

fn validate_private_dir(dir: &tempfile::TempDir) {
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700))
        .expect("set private test runtime directory permissions");

    let metadata = fs::metadata(dir.path()).expect("inspect private test runtime directory");
    assert_eq!(metadata.uid(), super::effective_uid());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
}

#[test]
fn private_runtime_tempdir_rejects_existing_relative_xdg_candidate() {
    assert!(private_tempdir_in(Path::new("."), "forktty-relative-").is_none());
}

struct TestXdgRuntimeEnv {
    previous: Option<OsString>,
    _guard: super::EnvTestLockGuard,
}

impl TestXdgRuntimeEnv {
    fn replace(value: Option<&OsStr>) -> Self {
        let guard = super::lock_env_for_test();
        let previous = std::env::var_os("XDG_RUNTIME_DIR");
        match value {
            Some(value) => unsafe { std::env::set_var("XDG_RUNTIME_DIR", value) },
            None => unsafe { std::env::remove_var("XDG_RUNTIME_DIR") },
        }
        Self {
            previous,
            _guard: guard,
        }
    }
}

impl Drop for TestXdgRuntimeEnv {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => unsafe { std::env::set_var("XDG_RUNTIME_DIR", value) },
            None => unsafe { std::env::remove_var("XDG_RUNTIME_DIR") },
        }
    }
}

#[test]
fn synthetic_runtime_is_effective_xdg_and_restores_unset_relative_and_missing_values() {
    let missing = std::env::temp_dir().join(format!(
        "forktty-missing-xdg-runtime-{}",
        std::process::id()
    ));
    let cases = [None, Some(OsStr::new(".")), Some(missing.as_os_str())];

    for original in cases {
        let env = TestXdgRuntimeEnv::replace(original);
        let fixture = private_runtime_tempdir("forktty-xdg-restore-");
        let effective = std::env::var_os("XDG_RUNTIME_DIR").expect("effective XDG runtime");
        assert!(Path::new(&effective).is_absolute());
        assert!(fixture.path().starts_with(Path::new(&effective)));
        drop(fixture);
        assert_eq!(std::env::var_os("XDG_RUNTIME_DIR").as_deref(), original);
        drop(env);
    }
}

#[test]
fn synthetic_runtime_restores_xdg_when_socket_directory_creation_panics() {
    let env = TestXdgRuntimeEnv::replace(Some(OsStr::new(".")));
    let overlong_prefix = "x".repeat(4_096);

    let result = std::panic::catch_unwind(|| private_runtime_tempdir(&overlong_prefix));

    assert!(result.is_err());
    assert_eq!(
        std::env::var_os("XDG_RUNTIME_DIR").as_deref(),
        Some(OsStr::new("."))
    );
    drop(env);
}
