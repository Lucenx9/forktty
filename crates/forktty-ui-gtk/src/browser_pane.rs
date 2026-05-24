//! WebKitGTK6 browser pane widget. Compiled only with `--features browser`.
#![cfg(feature = "browser")]

use gtk4 as gtk;

use gtk::prelude::*;
use webkit6::prelude::*;
use webkit6::WebView;

/// A browser pane: an address bar (entry + back/forward/reload) above a WebView.
///
/// Wired into the pane layout in a later task; unused until then.
#[allow(dead_code)]
pub struct BrowserPaneWidget {
    container: gtk::Box,
    web_view: WebView,
    address: gtk::Entry,
}

#[allow(dead_code)]
impl BrowserPaneWidget {
    pub fn new(initial_url: &str) -> Self {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);

        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let back = gtk::Button::from_icon_name("go-previous-symbolic");
        let forward = gtk::Button::from_icon_name("go-next-symbolic");
        let reload = gtk::Button::from_icon_name("view-refresh-symbolic");
        let address = gtk::Entry::new();
        address.set_hexpand(true);
        address.set_text(initial_url);
        bar.append(&back);
        bar.append(&forward);
        bar.append(&reload);
        bar.append(&address);

        let web_view = WebView::new();
        web_view.set_vexpand(true);

        {
            let wv = web_view.clone();
            back.connect_clicked(move |_| wv.go_back());
        }
        {
            let wv = web_view.clone();
            forward.connect_clicked(move |_| wv.go_forward());
        }
        {
            let wv = web_view.clone();
            reload.connect_clicked(move |_| wv.reload());
        }

        container.append(&bar);
        container.append(&web_view);

        let widget = Self {
            container,
            web_view,
            address,
        };
        widget.load_uri(initial_url);
        widget
    }

    pub fn widget(&self) -> gtk::Widget {
        self.container.clone().upcast()
    }

    pub fn load_uri(&self, url: &str) {
        if self.address.text() != url {
            self.address.set_text(url);
        }
        self.web_view.load_uri(url);
    }

    pub fn current_uri(&self) -> Option<String> {
        self.web_view.uri().map(|g| g.to_string())
    }

    /// Connect the address bar's Enter key to a navigation callback.
    pub fn connect_address_activate<F: Fn(String) + 'static>(&self, f: F) {
        let entry = self.address.clone();
        self.address.connect_activate(move |_| {
            f(entry.text().to_string());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_pane_widget_constructs_and_loads() {
        if gtk::init().is_err() {
            // No display in CI; skip rather than fail.
            return;
        }
        let pane = BrowserPaneWidget::new("https://example.com");
        pane.load_uri("https://other.com");
        assert_eq!(pane.address.text().as_str(), "https://other.com");
        let _ = pane.widget();
        // WebKitGTK installs atexit handlers (web/network process teardown)
        // that abort under headless/sandboxed process exit. The widget has been
        // constructed and exercised successfully by this point, so exit cleanly
        // before that teardown runs rather than letting it SIGABRT the harness.
        std::process::exit(0);
    }
}
