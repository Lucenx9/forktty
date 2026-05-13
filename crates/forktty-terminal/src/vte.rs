use crate::{SpawnRequest, TerminalError};

#[cfg(feature = "vte")]
use gtk4::glib;
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
    let terminal = vte4::Terminal::new();
    terminal.set_hexpand(true);
    terminal.set_vexpand(true);
    terminal.set_scrollback_lines(20_000);

    let env_storage = request
        .forktty_env()
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>();
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
        |result| {
            if let Err(err) = result {
                eprintln!("Failed to spawn VTE terminal: {err}");
            }
        },
    );

    Ok(terminal)
}

#[cfg(feature = "vte")]
pub fn send_text(widget: &VteTerminalWidget, text: &str) {
    widget.feed_child(text.as_bytes());
}
