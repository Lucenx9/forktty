use crate::{SpawnRequest, TerminalError};

#[cfg(feature = "vte")]
use gtk4::glib;
#[cfg(feature = "vte")]
use std::collections::BTreeMap;
#[cfg(feature = "vte")]
pub use vte4::prelude::TerminalExt;
#[cfg(feature = "vte")]
use vte4::prelude::*;
#[cfg(feature = "vte")]
pub use vte4::Format;

#[cfg(feature = "vte")]
pub type VteTerminalWidget = vte4::Terminal;

#[cfg(feature = "vte")]
pub fn spawn_vte_terminal(request: &SpawnRequest) -> Result<VteTerminalWidget, TerminalError> {
    spawn_vte_terminal_with_callback(request, |result| {
        if let Err(err) = result {
            eprintln!("Failed to spawn VTE terminal: {err}");
        }
    })
}

#[cfg(feature = "vte")]
pub fn spawn_vte_terminal_with_callback<F>(
    request: &SpawnRequest,
    on_spawn_result: F,
) -> Result<VteTerminalWidget, TerminalError>
where
    F: FnOnce(Result<glib::Pid, glib::Error>) + 'static,
{
    let terminal = vte4::Terminal::new();
    terminal.set_hexpand(true);
    terminal.set_vexpand(true);
    terminal.set_scrollback_lines(20_000);
    terminal.set_enable_legacy_osc777(true);

    let env_storage = child_environment(request);
    let envv = env_storage.iter().map(String::as_str).collect::<Vec<_>>();
    let argv = [request.shell.as_str()];
    let cwd_storage = request.cwd.to_string_lossy().to_string();

    terminal.spawn_async(
        vte4::PtyFlags::DEFAULT,
        Some(cwd_storage.as_str()),
        &argv,
        &envv,
        glib::SpawnFlags::DEFAULT,
        || {},
        -1,
        None::<&gtk4::gio::Cancellable>,
        on_spawn_result,
    );

    Ok(terminal)
}

#[cfg(feature = "vte")]
pub fn send_text(widget: &VteTerminalWidget, text: &str) {
    widget.feed_child(text.as_bytes());
}

#[cfg(feature = "vte")]
fn child_environment(request: &SpawnRequest) -> Vec<String> {
    let mut env = std::env::vars().collect::<BTreeMap<_, _>>();
    for (key, value) in request.forktty_env() {
        env.insert(key, value);
    }
    env.into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect()
}

#[cfg(all(test, feature = "vte"))]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn child_environment_inherits_and_overrides_forktty_vars() {
        let request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: vec![("PATH".to_string(), "/custom/bin".to_string())],
        };

        let env = child_environment(&request);

        assert!(env.iter().any(|entry| entry == "PATH=/custom/bin"));
        assert!(env
            .iter()
            .any(|entry| entry == "FORKTTY_WORKSPACE_ID=workspace-1"));
        assert!(env
            .iter()
            .any(|entry| entry == "FORKTTY_SURFACE_ID=surface-1"));
        assert!(env
            .iter()
            .any(|entry| entry == "FORKTTY_SOCKET_PATH=/tmp/forktty.sock"));
    }
}
