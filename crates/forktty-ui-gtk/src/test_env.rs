use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());
#[cfg(feature = "gtk-ghostty")]
static GTK_LOCK: Mutex<()> = Mutex::new(());

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

#[cfg(feature = "gtk-ghostty")]
pub(crate) fn with_gtk_test<T>(f: impl FnOnce() -> T) -> Option<T> {
    // GitHub's headless runner can report successful GTK initialization and
    // still segfault during widget construction. Keep those tests opt-in there.
    if std::env::var_os("FORKTTY_SKIP_GTK_WIDGET_TESTS").is_some() {
        return None;
    }

    let _guard = GTK_LOCK.lock().unwrap();
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
    match result {
        Ok(value) => Some(value),
        Err(payload) => resume_unwind(payload),
    }
}
