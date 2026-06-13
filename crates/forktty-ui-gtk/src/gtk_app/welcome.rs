use super::*;
use gtk::glib;
use gtk4 as gtk;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const WELCOME_MARKER_FILE: &str = "welcome-seen.json";
const SETUP_POLL_INTERVAL: Duration = Duration::from_millis(150);
const PRIVACY_URL: &str = "https://github.com/Lucenx9/forktty/blob/main/PRIVACY.md";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WelcomeMarker {
    welcomed_version: String,
}

/// True on the very first launch (the welcome marker has not been written yet).
///
/// When the XDG base dir cannot be resolved we return `false`: without a place
/// to record that the welcome was shown we would otherwise re-show it on every
/// launch, which is worse than skipping it.
pub(super) fn welcome_pending() -> bool {
    welcome_marker_path()
        .map(|path| !path.exists())
        .unwrap_or(false)
}

fn welcome_marker_path() -> Option<PathBuf> {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|base| base.join("forktty").join(WELCOME_MARKER_FILE))
}

/// Present the one-time welcome dialog: an informed telemetry notice plus a
/// one-click agent-integration setup. The first anonymous ping is deferred to
/// dialog dismissal so the user always sees the (default-on) toggle first.
pub(super) fn show_welcome_dialog(parent: &adw::ApplicationWindow, telemetry_enabled: bool) {
    let dialog = adw::MessageDialog::builder()
        .transient_for(parent)
        .modal(true)
        .heading("Welcome to ForkTTY")
        .body(
            "ForkTTY is a terminal multiplexer built for coding agents: embedded \
             terminals, git worktree workflows, and agent hook integration.",
        )
        .build();

    let content = gtk::Box::new(gtk::Orientation::Vertical, 14);

    let privacy_list = gtk::ListBox::new();
    privacy_list.add_css_class("boxed-list");
    privacy_list.set_selection_mode(gtk::SelectionMode::None);
    let ping_row = adw::SwitchRow::builder()
        .title("Anonymous daily ping")
        .subtitle(
            "One daily ping with app version and date only — no install id or project \
             data. Change this any time in Settings.",
        )
        .active(telemetry_enabled)
        .build();
    ping_row.connect_active_notify(|row| set_anonymous_ping(row.is_active()));
    privacy_list.append(&ping_row);
    content.append(&privacy_list);

    // A real, openable link (GtkLabel opens http(s) hrefs via its default
    // activate-link handler), so the consent notice is legible, not just text.
    let privacy_link = gtk::Label::new(None);
    privacy_link.set_markup(&format!(
        "<a href=\"{PRIVACY_URL}\">Read the ForkTTY privacy notice</a>"
    ));
    privacy_link.set_xalign(0.0);
    content.append(&privacy_link);

    let setup_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let setup_button = gtk::Button::with_label("Set up agent integration (hooks + MCP)");
    setup_button.add_css_class("suggested-action");
    let setup_status = gtk::Label::new(Some(
        "Installs ForkTTY hooks and the MCP bridge for Codex, Claude Code, and Antigravity.",
    ));
    setup_status.set_wrap(true);
    setup_status.set_xalign(0.0);
    setup_status.add_css_class("dim-label");
    setup_button.connect_clicked({
        let setup_status = setup_status.clone();
        move |button| run_agent_setup(button, &setup_status)
    });
    setup_box.append(&setup_button);
    setup_box.append(&setup_status);
    content.append(&setup_box);

    let doctor_hint = gtk::Label::new(Some(
        "Tip: run `forktty doctor` in a terminal to inspect your setup.",
    ));
    doctor_hint.set_wrap(true);
    doctor_hint.set_xalign(0.0);
    doctor_hint.add_css_class("dim-label");
    content.append(&doctor_hint);

    dialog.set_extra_child(Some(&content));
    dialog.add_response("start", "Get started");
    dialog.set_default_response(Some("start"));
    dialog.set_close_response("start");
    dialog.connect_response(None, {
        let ping_row = ping_row.clone();
        move |dialog, _| {
            record_welcome_seen_best_effort();
            // The startup ping was deferred on first launch; start it now that
            // the toggle has been seen. Consent comes from the live switch, not
            // a disk re-read: if persisting the preference failed, the on-disk
            // value could still be the default-on while the user switched it
            // off — the visible toggle is the source of truth for this dialog.
            if ping_row.is_active() {
                let mut config = config::load_config().unwrap_or_default();
                config.telemetry.anonymous_ping = true;
                crate::telemetry::maybe_start_anonymous_ping(&config);
            }
            dialog.close();
        }
    });
    dialog.present();
}

fn set_anonymous_ping(enabled: bool) {
    let base = config::load_config().unwrap_or_default();
    if base.telemetry.anonymous_ping == enabled {
        return;
    }
    let mut next = base;
    next.telemetry.anonymous_ping = enabled;
    if let Err(err) = config::save_config(&next) {
        eprintln!("forktty: could not save telemetry preference: {err}");
    }
}

fn run_agent_setup(button: &gtk::Button, status: &gtk::Label) {
    button.set_sensitive(false);
    status.remove_css_class("error");
    status.set_text("Configuring agent hooks and MCP…");

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run_setup_subcommands());
    });

    let button = button.clone();
    let status = status.clone();
    glib::timeout_add_local(SETUP_POLL_INTERVAL, move || match rx.try_recv() {
        Ok(Ok(())) => {
            status.set_text("✓ Agent hooks and MCP bridge configured.");
            glib::ControlFlow::Break
        }
        Ok(Err(err)) => {
            status.add_css_class("error");
            status.set_text(&format!(
                "Setup failed: {err}\nRun `forktty hooks setup` and `forktty mcp setup` in a terminal."
            ));
            button.set_sensitive(true);
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => {
            button.set_sensitive(true);
            glib::ControlFlow::Break
        }
    });
}

/// Run `forktty hooks setup` and `forktty mcp setup` against this same binary.
/// Both subcommands are dispatched by the CLI layer before any GUI launch, so
/// they write their config files and exit without touching the socket server.
fn run_setup_subcommands() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    run_setup_subcommand(&exe, &["hooks", "setup"])?;
    run_setup_subcommand(&exe, &["mcp", "setup"])?;
    Ok(())
}

fn run_setup_subcommand(exe: &Path, args: &[&str]) -> Result<(), String> {
    let output = Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| err.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail.trim();
    if detail.is_empty() {
        Err(format!("`forktty {}` failed", args.join(" ")))
    } else {
        Err(format!("`forktty {}`: {detail}", args.join(" ")))
    }
}

fn record_welcome_seen_best_effort() {
    if let Some(path) = welcome_marker_path() {
        if let Err(err) = record_welcome_seen(&path, env!("CARGO_PKG_VERSION")) {
            eprintln!("forktty: could not record welcome marker: {err}");
        }
    }
}

fn record_welcome_seen(path: &Path, version: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let marker = WelcomeMarker {
        welcomed_version: version.to_string(),
    };
    let bytes = serde_json::to_vec_pretty(&marker)?;
    let tmp_path = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut tmp = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)?;
        tmp.write_all(&bytes)?;
        tmp.sync_all()?;
        fs::rename(&tmp_path, path)?;
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_is_absent_then_present_after_recording() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("welcome-seen.json");

        assert!(!path.exists());

        record_welcome_seen(&path, "0.2.0-alpha.12").expect("record marker");

        assert!(path.exists());
        let marker: WelcomeMarker =
            serde_json::from_slice(&fs::read(&path).expect("read")).expect("parse");
        assert_eq!(marker.welcomed_version, "0.2.0-alpha.12");
    }

    #[test]
    fn recording_is_idempotent_and_overwrites_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("welcome-seen.json");

        record_welcome_seen(&path, "0.2.0-alpha.11").expect("first record");
        record_welcome_seen(&path, "0.2.0-alpha.12").expect("second record");

        let marker: WelcomeMarker =
            serde_json::from_slice(&fs::read(&path).expect("read")).expect("parse");
        assert_eq!(marker.welcomed_version, "0.2.0-alpha.12");
    }
}
