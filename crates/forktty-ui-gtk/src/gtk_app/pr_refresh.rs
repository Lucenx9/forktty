//! Pull-request hint refresh for workspace sidebar metadata.

use super::*;

struct AtomicBoolReset {
    flag: Arc<AtomicBool>,
}

impl Drop for AtomicBoolReset {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

fn pr_lookup_enabled() -> bool {
    config::load_config()
        .map(|config| config.general.enable_pr_lookup)
        .unwrap_or(false)
}

pub(super) fn clear_pr_hints(model: &Arc<Mutex<WorkspaceModel>>) {
    let Ok(mut model) = model.lock() else {
        return;
    };
    let workspace_ids = model
        .list_workspaces()
        .into_iter()
        .map(|workspace| workspace.id)
        .collect::<Vec<_>>();
    for workspace_id in workspace_ids {
        model.set_pr(&workspace_id, None);
    }
}

fn trusted_command_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable_file(candidate))
}

fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => return None,
        }
    }
}

fn child_stdout(mut child: std::process::Child) -> Option<String> {
    let stdout = child.stdout.take()?;
    let mut buffer = String::new();
    stdout
        .take(GH_PR_VIEW_MAX_STDOUT_BYTES)
        .read_to_string(&mut buffer)
        .ok()?;
    Some(buffer)
}

fn run_gh_pr_view(dir: &Path) -> Option<String> {
    let gh = trusted_command_path("gh")?;
    let mut child = Command::new(gh)
        .args(["pr", "view", "--json", "number,state,isDraft,url"])
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let status = wait_with_timeout(&mut child, GH_PR_VIEW_TIMEOUT)?;
    if !status.success() {
        return None;
    }
    child_stdout(child)
}

/// Kick off a background PR refresh unless one is already running.
pub(super) fn spawn_pr_refresh(model: Arc<Mutex<WorkspaceModel>>, in_flight: Arc<AtomicBool>) {
    if in_flight.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let _reset = AtomicBoolReset { flag: in_flight };
        refresh_pull_requests(model);
    });
}

/// Resolve each workspace's linked PR via `gh`, writing results into the shared
/// model. `gh` makes a network call, so this runs on a worker thread, never the
/// GTK main loop.
fn refresh_pull_requests(model: Arc<Mutex<WorkspaceModel>>) {
    if !pr_lookup_enabled() {
        clear_pr_hints(&model);
        return;
    }
    let targets: Vec<(String, PathBuf)> = match model.lock() {
        Ok(model) => model
            .list_workspaces()
            .into_iter()
            .map(|workspace| (workspace.id, workspace.working_dir))
            .collect(),
        Err(_) => return,
    };
    for (workspace_id, working_dir) in targets {
        let pr = resolve_pr(&working_dir);
        if !pr_lookup_enabled() {
            clear_pr_hints(&model);
            return;
        }
        if let Ok(mut model) = model.lock() {
            model.set_pr(&workspace_id, pr);
        }
    }
}

/// Run `gh pr view` in `dir` and parse the result. Returns `None` when there is
/// no PR for the checked-out branch, `gh` is absent, or `dir` is not a GitHub
/// checkout.
fn resolve_pr(dir: &Path) -> Option<forktty_core::pr::PrInfo> {
    let stdout = run_gh_pr_view(dir)?;
    forktty_core::pr::parse_pr_view(&stdout)
}
