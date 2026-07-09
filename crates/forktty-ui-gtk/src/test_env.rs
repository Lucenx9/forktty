use std::os::unix::fs::PermissionsExt;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());
#[cfg(feature = "gtk-ghostty")]
static GTK_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn with_env<T>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> T) -> T {
    let guard = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let saved = vars
        .iter()
        .map(|(key, _)| ((*key).to_string(), std::env::var_os(key)))
        .collect::<Vec<_>>();
    for (key, value) in vars {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
    let result = catch_unwind(AssertUnwindSafe(f));
    for (key, value) in saved {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
    drop(guard);
    match result {
        Ok(value) => value,
        Err(payload) => resume_unwind(payload),
    }
}

pub(crate) fn with_isolated_user_dirs<T>(f: impl FnOnce() -> T) -> T {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let state = root.path().join("state");
    let data = root.path().join("data");
    let config = root.path().join("config");
    let cache = root.path().join("cache");
    let runtime = root.path().join("runtime");
    for path in [&home, &state, &data, &config, &cache, &runtime] {
        std::fs::create_dir_all(path).unwrap();
    }
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
    let home = home.to_string_lossy().into_owned();
    let state = state.to_string_lossy().into_owned();
    let data = data.to_string_lossy().into_owned();
    let config = config.to_string_lossy().into_owned();
    let cache = cache.to_string_lossy().into_owned();
    let runtime = runtime.to_string_lossy().into_owned();
    with_env(
        &[
            ("HOME", Some(home.as_str())),
            ("XDG_STATE_HOME", Some(state.as_str())),
            ("XDG_DATA_HOME", Some(data.as_str())),
            ("XDG_CONFIG_HOME", Some(config.as_str())),
            ("XDG_CACHE_HOME", Some(cache.as_str())),
            ("XDG_RUNTIME_DIR", Some(runtime.as_str())),
        ],
        f,
    )
}

pub(crate) fn with_current_dir<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    let guard = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let saved = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir).unwrap();
    let result = catch_unwind(AssertUnwindSafe(f));
    std::env::set_current_dir(saved).unwrap();
    drop(guard);
    match result {
        Ok(value) => value,
        Err(payload) => resume_unwind(payload),
    }
}

#[cfg(feature = "gtk-ghostty")]
pub(crate) fn with_gtk_test<T>(f: impl FnOnce() -> T) -> Option<T> {
    // GitHub's headless runner can report successful GTK initialization and
    // still segfault during widget construction. Keep those tests opt-in there.
    if std::env::var_os("FORKTTY_SKIP_GTK_WIDGET_TESTS").is_some() {
        return None;
    }

    let guard = GTK_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // gtk-rs records the initializing thread; widget tests on other test
    // worker threads must skip instead of calling gtk::init again.
    if gtk4::is_initialized() {
        if !gtk4::is_initialized_main_thread() {
            return None;
        }
    } else if gtk4::init().is_err() {
        return None;
    }

    let result = catch_unwind(AssertUnwindSafe(f));
    drop(guard);
    match result {
        Ok(value) => Some(value),
        Err(payload) => resume_unwind(payload),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_env_restores_values_without_poisoning_lock_after_panic() {
        const KEY: &str = "FORKTTY_TEST_ENV_PANIC_RESTORE";
        let saved = std::env::var_os(KEY);

        let panicked = catch_unwind(AssertUnwindSafe(|| {
            with_env(&[(KEY, Some("during"))], || {
                assert_eq!(std::env::var(KEY).unwrap(), "during");
                panic!("expected test panic");
            });
        }));

        assert!(panicked.is_err());
        assert!(!ENV_LOCK.is_poisoned());
        assert_eq!(std::env::var_os(KEY), saved);
        let observed = with_env(&[(KEY, Some("after"))], || std::env::var(KEY).unwrap());
        assert_eq!(observed, "after");
        assert_eq!(std::env::var_os(KEY), saved);
    }

    #[test]
    fn with_current_dir_restores_directory_without_poisoning_lock_after_panic() {
        let saved = std::env::current_dir().unwrap();
        let dir = tempfile::tempdir().unwrap();

        let panicked = catch_unwind(AssertUnwindSafe(|| {
            with_current_dir(dir.path(), || panic!("expected test panic"));
        }));

        assert!(panicked.is_err());
        assert!(!ENV_LOCK.is_poisoned());
        assert_eq!(std::env::current_dir().unwrap(), saved);
    }

    #[test]
    fn with_isolated_user_dirs_redirects_all_user_directories() {
        with_isolated_user_dirs(|| {
            for key in [
                "HOME",
                "XDG_STATE_HOME",
                "XDG_DATA_HOME",
                "XDG_CONFIG_HOME",
                "XDG_CACHE_HOME",
                "XDG_RUNTIME_DIR",
            ] {
                assert!(std::path::Path::new(&std::env::var(key).unwrap()).is_dir());
            }
            let runtime = std::fs::metadata(std::env::var("XDG_RUNTIME_DIR").unwrap()).unwrap();
            assert_eq!(runtime.permissions().mode() & 0o777, 0o700);
        });
    }
}
