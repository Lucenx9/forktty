use adw::prelude::*;
use gtk::glib;
use gtk4 as gtk;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use super::ghostty_gtk_embed::GhosttyGtkEmbedder;
use super::{install_gtk_runtime_defaults, APP_ID};

const GHOSTTY_GTK_PROBE_EXIT_AFTER_MS_ENV: &str = "FORKTTY_GHOSTTY_GTK_PROBE_EXIT_AFTER_MS";

pub(super) fn run() -> i32 {
    let auto_exit_delay = match probe_auto_exit_delay() {
        Ok(delay) => delay,
        Err(err) => {
            eprintln!("forktty ghostty-gtk-probe: {err}");
            return 2;
        }
    };
    let exit_status = Rc::new(Cell::new(0));
    let exit_status_for_activate = Rc::clone(&exit_status);

    install_gtk_runtime_defaults();
    let app = adw::Application::builder()
        .application_id(format!("{APP_ID}.GhosttyGtkProbe"))
        .build();
    app.connect_activate(move |app| {
        build_probe_ui(app, auto_exit_delay, Rc::clone(&exit_status_for_activate));
    });
    let gtk_status: i32 = app.run_with_args(&probe_gapplication_args()).into();
    if gtk_status == 0 {
        exit_status.get()
    } else {
        gtk_status
    }
}

fn build_probe_ui(
    app: &adw::Application,
    auto_exit_delay: Option<Duration>,
    exit_status: Rc<Cell<i32>>,
) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("ForkTTY Ghostty GTK Probe")
        .default_width(960)
        .default_height(640)
        .build();

    match unsafe { GhosttyGtkEmbedder::load() } {
        Ok(probe) => match unsafe { probe.create_widget() } {
            Ok(widget) => {
                widget.set_hexpand(true);
                widget.set_vexpand(true);
                window.set_content(Some(&widget));

                let alive = Rc::new(Cell::new(true));
                let probe = Rc::new(probe);
                let probe_for_tick = Rc::clone(&probe);
                let alive_for_tick = Rc::clone(&alive);
                glib::timeout_add_local(Duration::from_millis(16), move || {
                    if !alive_for_tick.get() {
                        return glib::ControlFlow::Break;
                    }
                    unsafe {
                        probe_for_tick.tick();
                    }
                    glib::ControlFlow::Continue
                });

                window.connect_close_request(move |window| {
                    alive.set(false);
                    window.set_content(None::<&gtk::Widget>);
                    glib::Propagation::Proceed
                });
            }
            Err(err) => {
                exit_status.set(1);
                window.set_content(Some(&error_content(&err)));
            }
        },
        Err(err) => {
            exit_status.set(1);
            window.set_content(Some(&error_content(&err)));
        }
    }

    schedule_auto_exit(app, auto_exit_delay);
    window.present();
}

fn schedule_auto_exit(app: &adw::Application, delay: Option<Duration>) {
    if let Some(delay) = delay {
        let app = app.clone();
        glib::timeout_add_local_once(delay, move || {
            app.quit();
        });
    }
}

fn error_content(message: &str) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
    root.set_margin_top(24);
    root.set_margin_bottom(24);
    root.set_margin_start(24);
    root.set_margin_end(24);

    let title = gtk::Label::new(Some("Ghostty GTK probe failed"));
    title.add_css_class("title-2");
    title.set_xalign(0.0);

    let body = gtk::Label::new(Some(message));
    body.set_wrap(true);
    body.set_selectable(true);
    body.set_xalign(0.0);

    root.append(&title);
    root.append(&body);
    root
}

fn probe_auto_exit_delay() -> Result<Option<Duration>, String> {
    let Some(value) = std::env::var_os(GHOSTTY_GTK_PROBE_EXIT_AFTER_MS_ENV) else {
        return Ok(None);
    };
    let value = value.to_str().ok_or_else(|| {
        format!("{GHOSTTY_GTK_PROBE_EXIT_AFTER_MS_ENV} must be UTF-8 milliseconds")
    })?;
    parse_auto_exit_delay(value)
        .map_err(|err| format!("{GHOSTTY_GTK_PROBE_EXIT_AFTER_MS_ENV} {err}"))
}

fn parse_auto_exit_delay(value: &str) -> Result<Option<Duration>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let millis = value
        .parse::<u64>()
        .map_err(|_| "must be an integer number of milliseconds".to_string())?;
    if millis == 0 {
        Ok(None)
    } else {
        Ok(Some(Duration::from_millis(millis)))
    }
}

fn probe_gapplication_args() -> [&'static str; 1] {
    ["forktty-ghostty-gtk-probe"]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_auto_exit_delay_accepts_positive_millis() {
        assert_eq!(
            parse_auto_exit_delay("250").unwrap(),
            Some(Duration::from_millis(250))
        );
    }

    #[test]
    fn parse_auto_exit_delay_disables_on_zero_or_empty() {
        assert_eq!(parse_auto_exit_delay("0").unwrap(), None);
        assert_eq!(parse_auto_exit_delay("").unwrap(), None);
    }

    #[test]
    fn parse_auto_exit_delay_rejects_invalid_values() {
        assert!(parse_auto_exit_delay("soon").is_err());
    }

    #[test]
    fn probe_gapplication_args_do_not_forward_cli_subcommand() {
        assert_eq!(probe_gapplication_args(), ["forktty-ghostty-gtk-probe"]);
    }
}
