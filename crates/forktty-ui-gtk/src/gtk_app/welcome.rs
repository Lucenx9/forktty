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

/// Present the one-time welcome dialog: a branded hero, an informed telemetry
/// notice, and a one-click agent-integration setup. Built as a native
/// `.ft-dialog` window (matching the About dialog) rather than a generic
/// message dialog so it carries ForkTTY's own look. The first anonymous ping
/// is deferred to dialog dismissal so the user always sees the (default-on)
/// toggle first.
pub(super) fn show_welcome_dialog(parent: &adw::ApplicationWindow, telemetry_enabled: bool) {
    let window = gtk::Window::builder()
        .title("Welcome to ForkTTY")
        .transient_for(parent)
        .modal(true)
        .default_width(460)
        .resizable(false)
        .build();
    window.add_css_class("ft-dialog");
    window.add_css_class("welcome-dialog");
    apply_dialog_chrome(&window);
    install_escape_close(&window);
    restore_focus_after_hide(&window, parent);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("welcome-body");

    // Hero: logo + one-line value proposition, mirroring the About dialog.
    let hero = gtk::Box::new(gtk::Orientation::Vertical, 6);
    hero.add_css_class("welcome-hero");
    hero.set_halign(gtk::Align::Center);
    let logo = gtk::Image::from_icon_name("forktty");
    logo.set_pixel_size(72);
    logo.add_css_class("welcome-logo");
    let heading = gtk::Label::new(Some("Welcome to ForkTTY"));
    heading.add_css_class("welcome-heading");
    let subtitle = gtk::Label::builder()
        .label(
            "A terminal multiplexer built for coding agents — embedded terminals, \
             git worktrees, and agent hook integration.",
        )
        .wrap(true)
        .justify(gtk::Justification::Center)
        .max_width_chars(42)
        .build();
    subtitle.add_css_class("welcome-subtitle");
    hero.append(&logo);
    hero.append(&heading);
    hero.append(&subtitle);
    body.append(&hero);

    // Agent integration: the encouraged action gets the accent button.
    let setup_button = gtk::Button::with_label("Set up agent integration (hooks + MCP)");
    setup_button.add_css_class("suggested-action");
    setup_button.add_css_class("welcome-setup");
    let setup_status = gtk::Label::builder()
        .label(
            "Installs ForkTTY hooks for Codex, Claude Code, Antigravity, and OpenCode, \
             plus the MCP bridge for Codex, Claude Code, and Antigravity.",
        )
        .wrap(true)
        .justify(gtk::Justification::Center)
        .max_width_chars(44)
        .build();
    setup_status.add_css_class("welcome-setup-status");
    setup_button.connect_clicked({
        let setup_status = setup_status.clone();
        move |button| run_agent_setup(button, &setup_status)
    });
    let setup_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    setup_box.add_css_class("welcome-setup-box");
    setup_box.append(&setup_button);
    setup_box.append(&setup_status);
    body.append(&setup_box);

    // Telemetry consent: visible default-on toggle with an openable notice link.
    let privacy_list = gtk::ListBox::new();
    privacy_list.add_css_class("boxed-list");
    privacy_list.add_css_class("welcome-card");
    privacy_list.set_selection_mode(gtk::SelectionMode::None);
    let ping_row = adw::SwitchRow::builder()
        .title("Anonymous daily ping")
        .subtitle(
            "One daily ping with app version and date only — no install id or project \
             data. Change this any time in Settings.",
        )
        .active(telemetry_enabled)
        .build();
    let telemetry_status = gtk::Label::builder()
        .wrap(true)
        .justify(gtk::Justification::Center)
        .max_width_chars(44)
        .visible(false)
        .build();
    telemetry_status.add_css_class("welcome-telemetry-status");
    ping_row.connect_active_notify({
        let telemetry_status = telemetry_status.clone();
        move |row| match set_anonymous_ping(row.is_active()) {
            Ok(()) => clear_telemetry_status(&telemetry_status),
            Err(err) => show_telemetry_error(&telemetry_status, row.is_active(), &err),
        }
    });
    privacy_list.append(&ping_row);
    body.append(&privacy_list);

    // A real, openable link (GtkLabel opens http(s) hrefs via its default
    // activate-link handler), so the consent notice is legible, not just text.
    let privacy_link = gtk::Label::new(None);
    privacy_link.set_markup(&format!(
        "<a href=\"{PRIVACY_URL}\">Read the ForkTTY privacy notice</a>"
    ));
    privacy_link.add_css_class("welcome-link");
    privacy_link.set_halign(gtk::Align::Center);
    body.append(&privacy_link);
    body.append(&telemetry_status);

    // Footer: a clear way into the app. Closing by any route (button, the
    // titlebar close, or Escape) runs the same dismissal logic below.
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    footer.add_css_class("welcome-footer");
    footer.set_halign(gtk::Align::End);
    let get_started = gtk::Button::with_label("Get started");
    get_started.add_css_class("welcome-start");
    get_started.connect_clicked({
        let window = window.clone();
        move |_| window.close()
    });
    footer.append(&get_started);
    body.append(&footer);

    window.connect_close_request({
        let ping_row = ping_row.clone();
        let telemetry_status = telemetry_status.clone();
        move |_| {
            if !ping_row.is_active() {
                // Persist the opt-out, but never trap the user in the dialog if
                // the write fails (e.g. a read-only config dir): the live toggle
                // is the source of truth and we skip the ping below regardless.
                if let Err(err) = set_anonymous_ping(false) {
                    show_telemetry_error(&telemetry_status, false, &err);
                }
            }
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
            glib::Propagation::Proceed
        }
    });

    window.set_default_widget(Some(&get_started));
    window.set_child(Some(&body));
    window.present();
    get_started.grab_focus();
}

fn set_anonymous_ping(enabled: bool) -> Result<(), String> {
    let path = config::config_path().map_err(|err| err.to_string())?;
    set_anonymous_ping_at_path(&path, enabled)
}

fn set_anonymous_ping_at_path(path: &Path, enabled: bool) -> Result<(), String> {
    let base = config::load_config_from_path(path).unwrap_or_default();
    if base.telemetry.anonymous_ping == enabled {
        return Ok(());
    }
    let mut next = base;
    next.telemetry.anonymous_ping = enabled;
    config::save_config_to_path(path, &next).map_err(|err| err.to_string())
}

fn clear_telemetry_status(status: &gtk::Label) {
    status.set_text("");
    status.set_visible(false);
}

fn show_telemetry_error(status: &gtk::Label, enabled: bool, error: &str) {
    let detail = if enabled {
        format!("Could not save telemetry preference: {error}.")
    } else {
        format!(
            "Could not save telemetry opt-out: {error}. Fix config permissions or turn the ping back on to continue."
        )
    };
    status.set_text(&detail);
    status.set_visible(true);
    if !status.has_css_class("error") {
        status.add_css_class("error");
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

    #[test]
    fn telemetry_preference_reports_persist_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let blocked_config_home = dir.path().join("not-a-directory");
        fs::write(&blocked_config_home, b"blocked").expect("blocked file");
        let path = blocked_config_home.join("config.toml");

        assert!(set_anonymous_ping_at_path(&path, false).is_err());
    }

    #[test]
    fn telemetry_preference_persists_opt_out() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");

        set_anonymous_ping_at_path(&path, false).expect("save opt-out");
        let config = config::load_config_from_path(&path).expect("load saved config");
        assert!(!config.telemetry.anonymous_ping);
    }
}
