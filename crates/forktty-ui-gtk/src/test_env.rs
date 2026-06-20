use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn with_env<T>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> T) -> T {
    let _guard = ENV_LOCK.lock().unwrap();
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
    match result {
        Ok(value) => value,
        Err(payload) => resume_unwind(payload),
    }
}

pub(crate) fn with_current_dir<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    let _guard = ENV_LOCK.lock().unwrap();
    let saved = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir).unwrap();
    let result = catch_unwind(AssertUnwindSafe(f));
    std::env::set_current_dir(saved).unwrap();
    match result {
        Ok(value) => value,
        Err(payload) => resume_unwind(payload),
    }
}
