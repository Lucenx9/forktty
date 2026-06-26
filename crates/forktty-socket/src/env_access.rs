//! Test-only environment access helpers.

use std::ffi::OsString;
use std::sync::{Condvar, Mutex, OnceLock};
use std::thread::ThreadId;

static ENV_TEST_LOCK: OnceLock<EnvTestLock> = OnceLock::new();

struct EnvTestLock {
    state: Mutex<EnvTestLockState>,
    cvar: Condvar,
}

#[derive(Default)]
struct EnvTestLockState {
    owner: Option<ThreadId>,
    depth: usize,
}

pub(crate) struct EnvTestLockGuard {
    lock: &'static EnvTestLock,
}

pub(crate) fn lock_env_for_test() -> EnvTestLockGuard {
    let lock = ENV_TEST_LOCK.get_or_init(|| EnvTestLock {
        state: Mutex::new(EnvTestLockState::default()),
        cvar: Condvar::new(),
    });
    let current = std::thread::current().id();
    let mut state = lock.state.lock().expect("env test lock poisoned");
    loop {
        match state.owner {
            Some(owner) if owner == current => {
                state.depth += 1;
                return EnvTestLockGuard { lock };
            }
            Some(_) => {
                state = lock.cvar.wait(state).expect("env test lock poisoned");
            }
            None => {
                state.owner = Some(current);
                state.depth = 1;
                return EnvTestLockGuard { lock };
            }
        }
    }
}

impl Drop for EnvTestLockGuard {
    fn drop(&mut self) {
        let current = std::thread::current().id();
        let mut state = self.lock.state.lock().expect("env test lock poisoned");
        debug_assert_eq!(state.owner, Some(current));
        state.depth = state.depth.saturating_sub(1);
        if state.depth == 0 {
            state.owner = None;
            self.lock.cvar.notify_all();
        }
    }
}

pub(crate) fn with_env_read_lock<T>(f: impl FnOnce() -> T) -> T {
    let _guard = lock_env_for_test();
    f()
}

pub(crate) fn var_os(key: &str) -> Option<OsString> {
    with_env_read_lock(|| std::env::var_os(key))
}

pub(crate) fn var(key: &str) -> Result<String, std::env::VarError> {
    with_env_read_lock(|| std::env::var(key))
}
