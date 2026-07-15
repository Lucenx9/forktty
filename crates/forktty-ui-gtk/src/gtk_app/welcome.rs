use super::*;
use gtk::glib;
use gtk4 as gtk;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const WELCOME_MARKER_FILE: &str = "welcome-seen.json";
const PRIVACY_URL: &str = "https://forktty.dev/privacy";

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
/// notice, and a direct path into agent-integration settings. Built as a native
/// `.ft-dialog` window (matching the About dialog) rather than a generic
/// message dialog so it carries ForkTTY's own look. The first anonymous ping
/// is deferred to dialog dismissal so the user always sees the (default-on)
/// toggle first.
pub(super) fn show_welcome_dialog(
    parent: &adw::ApplicationWindow,
    telemetry_enabled: bool,
    open_agent_settings: Rc<dyn Fn()>,
) {
    let window = gtk::Window::builder()
        .title("Welcome to ForkTTY")
        .transient_for(parent)
        .modal(true)
        .default_width(430)
        .resizable(false)
        .build();
    window.add_css_class("ft-dialog");
    window.add_css_class("welcome-dialog");
    apply_dialog_chrome(&window);
    install_escape_close(&window);
    restore_focus_after_hide(&window, parent);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("welcome-body");

    // Hero: compact status-page style intro, without turning first launch into
    // a marketing screen.
    let hero = gtk::Box::new(gtk::Orientation::Vertical, 8);
    hero.add_css_class("welcome-hero");
    hero.set_halign(gtk::Align::Center);
    let logo = gtk::Image::from_icon_name("forktty");
    logo.set_pixel_size(52);
    logo.add_css_class("welcome-logo");
    let heading = gtk::Label::new(Some("Welcome to ForkTTY"));
    heading.add_css_class("welcome-heading");
    let subtitle = gtk::Label::builder()
        .label("Ghostty-powered workspaces for coding agents.")
        .wrap(true)
        .justify(gtk::Justification::Center)
        .max_width_chars(38)
        .build();
    subtitle.add_css_class("welcome-subtitle");
    hero.append(&logo);
    hero.append(&heading);
    hero.append(&subtitle);
    body.append(&hero);

    // Agent integration: compact callout, not a full preference row.
    let integration_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    integration_row.add_css_class("welcome-integration-row");
    integration_row.set_valign(gtk::Align::Center);
    let integration_text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    integration_text.set_hexpand(true);
    let integration_title = gtk::Label::builder()
        .label("Agent integrations")
        .xalign(0.0)
        .build();
    integration_title.add_css_class("welcome-integration-title");
    let integration_subtitle = gtk::Label::builder()
        .label("Lifecycle and notification hooks for supported agents.")
        .xalign(0.0)
        .build();
    integration_subtitle.add_css_class("welcome-integration-subtitle");
    integration_text.append(&integration_title);
    integration_text.append(&integration_subtitle);
    integration_row.append(&integration_text);
    let setup_button = gtk::Button::builder()
        .label("Set Up")
        .valign(gtk::Align::Center)
        .build();
    setup_button.add_css_class("suggested-action");
    setup_button.add_css_class("welcome-setup");
    integration_row.append(&setup_button);
    let setup_status = gtk::Label::builder()
        .label("Review install status or configure optional agent hooks.")
        .wrap(true)
        .justify(gtk::Justification::Left)
        .xalign(0.0)
        .max_width_chars(46)
        .build();
    setup_status.add_css_class("welcome-setup-status");
    setup_button.connect_clicked({
        let open_agent_settings = open_agent_settings.clone();
        let window = window.clone();
        move |_| {
            window.close();
            let open_agent_settings = open_agent_settings.clone();
            let window = window.clone();
            glib::idle_add_local_once(move || {
                if !window.is_visible() {
                    open_agent_settings();
                }
            });
        }
    });
    let setup_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    setup_box.add_css_class("welcome-setup-box");
    setup_box.append(&integration_row);
    setup_box.append(&setup_status);
    body.append(&setup_box);

    // Telemetry consent: visible default-on toggle with an openable notice link.
    let privacy_list = gtk::ListBox::new();
    privacy_list.add_css_class("boxed-list");
    privacy_list.add_css_class("welcome-card");
    privacy_list.set_selection_mode(gtk::SelectionMode::None);
    let ping_row = adw::SwitchRow::builder()
        .title("Anonymous daily ping")
        .subtitle("Version and date only. No install id or project data.")
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

    // Footer: a clear way into the app, with privacy as a secondary text link.
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    footer.add_css_class("welcome-footer");
    footer.set_valign(gtk::Align::Center);

    // A real, openable link (GtkLabel opens http(s) hrefs via its default
    // activate-link handler), so the consent notice is legible, not just text.
    let privacy_link = gtk::Label::new(None);
    privacy_link.set_markup(&format!("<a href=\"{PRIVACY_URL}\">Privacy notice</a>"));
    privacy_link.add_css_class("welcome-link");
    privacy_link.set_halign(gtk::Align::Start);
    privacy_link.set_hexpand(true);
    footer.append(&privacy_link);

    let get_started = gtk::Button::with_label("Get Started");
    get_started.add_css_class("welcome-start");
    get_started.add_css_class("pill");
    get_started.connect_clicked({
        let window = window.clone();
        move |_| window.close()
    });
    footer.append(&get_started);
    body.append(&telemetry_status);
    body.append(&footer);

    window.connect_close_request({
        let ping_row = ping_row.clone();
        let telemetry_status = telemetry_status.clone();
        move |_| {
            if let Err(err) = persist_welcome_telemetry_choice(ping_row.is_active()) {
                // Persist the opt-out before recording first-run completion. If
                // the write fails (e.g. a read-only config dir), keep the
                // welcome open so the user can fix permissions or re-enable
                // the ping before continuing.
                show_telemetry_error(&telemetry_status, false, &err);
                return glib::Propagation::Stop;
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

fn persist_welcome_telemetry_choice(enabled: bool) -> Result<(), String> {
    let path = config::config_path().map_err(|err| err.to_string())?;
    persist_welcome_telemetry_choice_at_path(&path, enabled)
}

fn persist_welcome_telemetry_choice_at_path(path: &Path, enabled: bool) -> Result<(), String> {
    if enabled {
        Ok(())
    } else {
        set_anonymous_ping_at_path(path, false)
    }
}

fn set_anonymous_ping_at_path(path: &Path, enabled: bool) -> Result<(), String> {
    let base = match config::load_config_from_path(path) {
        Ok(config) => config,
        Err(_) if !path.exists() => config::AppConfig::default(),
        Err(err) => return Err(err.to_string()),
    };
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
    fn welcome_opt_out_must_persist_before_completion() {
        let dir = tempfile::tempdir().expect("tempdir");
        let blocked_config_home = dir.path().join("not-a-directory");
        fs::write(&blocked_config_home, b"blocked").expect("blocked file");
        let path = blocked_config_home.join("config.toml");

        assert!(persist_welcome_telemetry_choice_at_path(&path, false).is_err());
    }

    #[test]
    fn welcome_default_ping_choice_does_not_require_config_write() {
        let dir = tempfile::tempdir().expect("tempdir");
        let blocked_config_home = dir.path().join("not-a-directory");
        fs::write(&blocked_config_home, b"blocked").expect("blocked file");
        let path = blocked_config_home.join("config.toml");

        persist_welcome_telemetry_choice_at_path(&path, true)
            .expect("default-on choice does not need to be written");
    }

    #[test]
    fn telemetry_preference_persists_opt_out() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");

        set_anonymous_ping_at_path(&path, false).expect("save opt-out");
        let config = config::load_config_from_path(&path).expect("load saved config");
        assert!(!config.telemetry.anonymous_ping);
    }

    #[test]
    fn telemetry_preference_refuses_to_overwrite_invalid_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(&path, b"not = [valid").expect("write invalid config");

        assert!(set_anonymous_ping_at_path(&path, false).is_err());
        assert_eq!(fs::read(&path).expect("read config"), b"not = [valid");
    }
}
