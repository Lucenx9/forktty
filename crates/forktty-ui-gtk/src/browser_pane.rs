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
    close: gtk::Button,
    /// The last url this widget was *asked* to load. The reload guard
    /// edge-triggers on this, not on the WebView's committed `current_uri()`,
    /// which diverges due to WebKit normalization, redirects, and user clicks.
    last_requested: std::cell::RefCell<String>,
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
        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.set_tooltip_text(Some("Close Pane"));
        close.add_css_class("pane-close-action");
        bar.append(&back);
        bar.append(&forward);
        bar.append(&reload);
        bar.append(&address);
        bar.append(&close);

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
            close,
            // Empty so the first load_uri(initial_url) below is not a no-op;
            // that single call populates last_requested and avoids a double-load.
            last_requested: std::cell::RefCell::new(String::new()),
        };
        widget.load_uri(initial_url);
        widget
    }

    pub fn widget(&self) -> gtk::Widget {
        self.container.clone().upcast()
    }

    /// The widget keyboard focus should land on when this pane is focused.
    /// The address entry is the natural keyboard entry point and is always
    /// realized (the WebView may not have loaded yet).
    pub fn focus_target(&self) -> gtk::Widget {
        self.address.clone().upcast()
    }

    /// Connect a callback fired when focus enters this pane (the address bar
    /// or the WebView). Used to keep `WorkspaceModel::focused_surface_id` in
    /// sync so split/close commands target the pane the user is interacting
    /// with, mirroring the VTE `has-focus` handler.
    pub fn connect_focus_in<F: Fn() + 'static>(&self, f: F) {
        let f = std::rc::Rc::new(f);
        for target in [
            self.web_view.clone().upcast::<gtk::Widget>(),
            self.address.clone().upcast::<gtk::Widget>(),
        ] {
            let controller = gtk::EventControllerFocus::new();
            let f = f.clone();
            controller.connect_enter(move |_| f());
            target.add_controller(controller);
        }
    }

    pub fn load_uri(&self, url: &str) {
        if self.last_requested.borrow().as_str() == url {
            return;
        }
        *self.last_requested.borrow_mut() = url.to_string();
        if self.address.text().as_str() != url {
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

    /// Connect the close (×) button to a callback. The widget does not own the
    /// model or the confirmation dialog, so gtk_app wires this to the same
    /// `show_close_pane_confirmation` path terminal panes use.
    pub fn connect_close<F: Fn() + 'static>(&self, f: F) {
        self.close.connect_clicked(move |_| f());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Constructs a live WebKitGTK WebView, which requires a display and whose
    // process-teardown aborts under WebKit. Marked #[ignore] so it stays out of
    // the default suite; run explicitly with:
    //   cargo test -p forktty-ui-gtk --features browser -- --ignored browser_pane_widget_constructs_and_loads
    #[test]
    #[ignore]
    fn browser_pane_widget_constructs_and_loads() {
        if gtk::init().is_err() {
            // No display in CI; skip rather than fail.
            return;
        }
        let pane = BrowserPaneWidget::new("https://example.com");
        assert_eq!(pane.last_requested.borrow().as_str(), "https://example.com");
        pane.load_uri("https://other.com");
        assert_eq!(pane.address.text().as_str(), "https://other.com");
        assert_eq!(pane.last_requested.borrow().as_str(), "https://other.com");
        // Self-guard: re-requesting the same url is a no-op. We can't observe the
        // WebView load count without a display, so assert it does not panic and
        // the address text / last_requested stay stable.
        pane.load_uri("https://other.com");
        assert_eq!(pane.address.text().as_str(), "https://other.com");
        assert_eq!(pane.last_requested.borrow().as_str(), "https://other.com");
        let _ = pane.widget();
    }
}
