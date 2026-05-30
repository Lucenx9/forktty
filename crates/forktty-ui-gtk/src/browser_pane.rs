//! WebKitGTK6 browser pane widget. Compiled only with `--features browser`.
#![cfg(feature = "browser")]

use gtk4 as gtk;

use gtk::glib;
use gtk::prelude::*;
use webkit6::prelude::*;
use webkit6::WebView;

use forktty_core::BrowserCmdError;

/// Return `true` only for `http://` and `https://` URLs — the ones worth recording in history.
fn is_recordable_url(url: &str) -> bool {
    has_url_scheme(url, "http://") || has_url_scheme(url, "https://")
}

fn has_url_scheme(url: &str, scheme: &str) -> bool {
    url.get(..scheme.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
}

fn sync_history_buttons(web_view: &WebView, back: &gtk::Button, forward: &gtk::Button) {
    back.set_sensitive(web_view.can_go_back());
    forward.set_sensitive(web_view.can_go_forward());
}

fn accessible_shortcut_text(shortcut: &str) -> String {
    shortcut
        .split('/')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            part.replace("Ctrl", "Control")
                .replace("Esc", "Escape")
                .replace("Control+,", "Control+comma")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn set_accessible_button_text(button: &gtk::Button, label: &str, shortcut: Option<&str>) {
    if let Some(shortcut) = shortcut {
        let shortcut = accessible_shortcut_text(shortcut);
        button.update_property(&[
            gtk::accessible::Property::Label(label),
            gtk::accessible::Property::KeyShortcuts(shortcut.as_str()),
        ]);
    } else {
        button.update_property(&[gtk::accessible::Property::Label(label)]);
    }
}

/// The scripting driver injected into every page (SP2).
pub const DRIVER_JS: &str = include_str!("driver.js");

/// Build the JS call for `window.__forktty.click(ref)`, JSON-quoting `reference`.
pub fn click_js(reference: &str) -> String {
    format!(
        "window.__forktty.click({})",
        serde_json::to_string(reference).unwrap_or_else(|_| "\"\"".to_string())
    )
}

/// Build the JS call for `window.__forktty.fill(ref, value)`, JSON-quoting both.
pub fn fill_js(reference: &str, value: &str) -> String {
    format!(
        "window.__forktty.fill({},{})",
        serde_json::to_string(reference).unwrap_or_else(|_| "\"\"".to_string()),
        serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
    )
}

#[derive(Clone)]
struct BrowserPaneNavigation {
    web_view: WebView,
    address: gtk::Entry,
    last_requested: std::rc::Rc<std::cell::RefCell<String>>,
}

type UriCommittedHandlers = std::rc::Rc<std::cell::RefCell<Vec<Box<dyn Fn(String)>>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BrowserPaneShortcut {
    FocusAddress,
    Back,
    Forward,
    Reload,
}

impl BrowserPaneNavigation {
    fn load_uri(&self, url: &str) {
        if self.last_requested.borrow().as_str() == url {
            return;
        }
        *self.last_requested.borrow_mut() = url.to_string();
        self.update_address(url);
        self.web_view.load_uri(url);
    }

    fn update_address(&self, url: &str) {
        if self.address.text().as_str() != url {
            self.address.set_text(url);
        }
    }
}

fn browser_pane_shortcut(
    key: gtk::gdk::Key,
    modifiers: gtk::gdk::ModifierType,
) -> Option<BrowserPaneShortcut> {
    let control = modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
    let alt = modifiers.contains(gtk::gdk::ModifierType::ALT_MASK);
    if control && matches!(key, gtk::gdk::Key::l | gtk::gdk::Key::L) {
        return Some(BrowserPaneShortcut::FocusAddress);
    }
    if alt && matches!(key, gtk::gdk::Key::d | gtk::gdk::Key::D) {
        return Some(BrowserPaneShortcut::FocusAddress);
    }
    if alt && key == gtk::gdk::Key::Left {
        return Some(BrowserPaneShortcut::Back);
    }
    if alt && key == gtk::gdk::Key::Right {
        return Some(BrowserPaneShortcut::Forward);
    }
    if (control && matches!(key, gtk::gdk::Key::r | gtk::gdk::Key::R)) || key == gtk::gdk::Key::F5 {
        return Some(BrowserPaneShortcut::Reload);
    }
    None
}

fn install_browser_shortcuts<W: IsA<gtk::Widget>>(
    target: &W,
    web_view: &WebView,
    address: &gtk::Entry,
) {
    let controller = gtk::EventControllerKey::new();
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let web_view = web_view.downgrade();
    let address = address.downgrade();
    controller.connect_key_pressed(move |_, key, _, modifiers| {
        let Some(web_view) = web_view.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let Some(address) = address.upgrade() else {
            return glib::Propagation::Proceed;
        };
        match browser_pane_shortcut(key, modifiers) {
            Some(BrowserPaneShortcut::FocusAddress) => {
                address.grab_focus();
                address.select_region(0, -1);
            }
            Some(BrowserPaneShortcut::Back) => {
                if web_view.can_go_back() {
                    web_view.go_back();
                }
            }
            Some(BrowserPaneShortcut::Forward) => {
                if web_view.can_go_forward() {
                    web_view.go_forward();
                }
            }
            Some(BrowserPaneShortcut::Reload) => {
                web_view.reload();
            }
            None => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    target.add_controller(controller);
}

/// A browser pane: an address bar (entry + back/forward/reload) above a WebView.
///
/// Wired into the pane layout in a later task; unused until then.
#[allow(dead_code)]
pub struct BrowserPaneWidget {
    container: gtk::Box,
    toolbar: gtk::Box,
    web_view: WebView,
    address: gtk::Entry,
    close: gtk::Button,
    /// The last url this widget was *asked* to load. The reload guard
    /// edge-triggers on this, not on the WebView's committed `current_uri()`,
    /// which diverges due to WebKit normalization, redirects, and user clicks.
    last_requested: std::rc::Rc<std::cell::RefCell<String>>,
    /// Callbacks fired for committed WebView URI changes (redirects, history
    /// navigation, and in-page link clicks). GTK wires this back into the model
    /// so periodic UI refreshes do not navigate the WebView back to a stale URL.
    committed_uri_handlers: UriCommittedHandlers,
}

#[allow(dead_code)]
impl BrowserPaneWidget {
    pub fn new(profile_id: &str, initial_url: &str) -> Self {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.add_css_class("browser-pane");

        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        bar.add_css_class("browser-toolbar");
        bar.update_property(&[gtk::accessible::Property::Label("Browser navigation")]);
        let back = gtk::Button::from_icon_name("forktty-back-symbolic");
        let forward = gtk::Button::from_icon_name("forktty-forward-symbolic");
        let reload = gtk::Button::from_icon_name("forktty-refresh-symbolic");
        back.set_sensitive(false);
        forward.set_sensitive(false);
        let address = gtk::Entry::new();
        address.set_hexpand(true);
        address.set_text(initial_url);
        address.set_placeholder_text(Some("Enter URL"));
        address.add_css_class("browser-address");
        address.set_tooltip_text(Some("Address bar (Ctrl+L)"));
        let address_shortcuts = accessible_shortcut_text("Ctrl+L / Alt+D");
        address.update_property(&[
            gtk::accessible::Property::Label("Address bar"),
            gtk::accessible::Property::KeyShortcuts(address_shortcuts.as_str()),
        ]);
        let close = gtk::Button::from_icon_name("forktty-close-symbolic");
        close.add_css_class("pane-close-action");
        for (button, label, shortcut, tooltip) in [
            (&back, "Back", Some("Alt+Left"), "Back (Alt+Left)"),
            (
                &forward,
                "Forward",
                Some("Alt+Right"),
                "Forward (Alt+Right)",
            ),
            (
                &reload,
                "Reload",
                Some("Ctrl+R / F5"),
                "Reload (Ctrl+R / F5)",
            ),
            (&close, "Close Pane", None, "Close Pane"),
        ] {
            button.add_css_class("flat");
            button.add_css_class("browser-action");
            button.set_tooltip_text(Some(tooltip));
            set_accessible_button_text(button, label, shortcut);
        }
        bar.append(&back);
        bar.append(&forward);
        bar.append(&reload);
        bar.append(&address);
        bar.append(&close);

        let session = crate::browser_session::session_for(profile_id);
        let web_view = WebView::builder().network_session(&session).build();
        web_view.set_vexpand(true);
        install_browser_shortcuts(&container, &web_view, &address);
        install_browser_shortcuts(&web_view, &web_view, &address);
        install_browser_shortcuts(&address, &web_view, &address);
        sync_history_buttons(&web_view, &back, &forward);

        {
            use webkit6::{UserContentInjectedFrames, UserScript, UserScriptInjectionTime};
            let content_manager = web_view
                .user_content_manager()
                .expect("WebView always has a default UserContentManager");
            let script = UserScript::new(
                DRIVER_JS,
                UserContentInjectedFrames::TopFrame,
                UserScriptInjectionTime::Start,
                &[],
                &[],
            );
            content_manager.add_script(&script);
        }

        let navigation = BrowserPaneNavigation {
            web_view: web_view.clone(),
            address: address.clone(),
            last_requested: std::rc::Rc::new(std::cell::RefCell::new(String::new())),
        };
        let committed_uri_handlers: UriCommittedHandlers =
            std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));

        {
            let wv = web_view.downgrade();
            let back_button = back.downgrade();
            let forward_button = forward.downgrade();
            back.connect_clicked(move |_| {
                let Some(wv) = wv.upgrade() else {
                    return;
                };
                let Some(back_button) = back_button.upgrade() else {
                    return;
                };
                let Some(forward_button) = forward_button.upgrade() else {
                    return;
                };
                wv.go_back();
                sync_history_buttons(&wv, &back_button, &forward_button);
            });
        }
        {
            let wv = web_view.downgrade();
            let back_button = back.downgrade();
            let forward_button = forward.downgrade();
            forward.connect_clicked(move |_| {
                let Some(wv) = wv.upgrade() else {
                    return;
                };
                let Some(back_button) = back_button.upgrade() else {
                    return;
                };
                let Some(forward_button) = forward_button.upgrade() else {
                    return;
                };
                wv.go_forward();
                sync_history_buttons(&wv, &back_button, &forward_button);
            });
        }
        {
            let wv = web_view.downgrade();
            reload.connect_clicked(move |_| {
                if let Some(wv) = wv.upgrade() {
                    wv.reload();
                }
            });
        }

        // Parse profile_id → ProfileId for history/bookmark stores.
        let profile = profile_id
            .parse::<forktty_core::ProfileId>()
            .unwrap_or_default();

        // Address-bar completion: a ListStore with one text column, populated from
        // history + bookmarks. Built before the load-changed connection so it can
        // be refreshed from there.
        let completion_model = gtk::ListStore::new(&[glib::Type::STRING]);
        let completion = gtk::EntryCompletion::new();
        completion.set_model(Some(&completion_model));
        completion.set_text_column(0);
        completion.set_inline_completion(true);
        completion.set_popup_completion(true);
        address.set_completion(Some(&completion));

        // Populate the completion model from history + bookmarks.
        let populate_completion = {
            let model = completion_model.clone();
            move |profile: forktty_core::ProfileId| {
                model.clear();
                // Collect URLs from history (most recent first) then bookmarks.
                let mut urls: Vec<String> = Vec::new();
                if let Ok(hist) = forktty_core::HistoryStore::for_profile(profile) {
                    if let Ok(visits) = hist.list(200) {
                        for v in visits {
                            urls.push(v.url);
                        }
                    }
                }
                if let Ok(bm) = forktty_core::BookmarkStore::for_profile(profile) {
                    for b in bm.list() {
                        if !urls.contains(&b.url) {
                            urls.push(b.url.clone());
                        }
                    }
                }
                for url in urls {
                    let iter = model.append();
                    model.set_value(&iter, 0, &url.to_value());
                }
            }
        };
        // Initial population.
        populate_completion(profile);

        // Connect load-changed: sync the committed URI into the toolbar, record
        // visits on Committed, and refresh completion. Updating last_requested
        // here prevents later UI refresh ticks from reloading the stale model URL
        // after redirects, history navigation, or in-page user clicks.
        {
            let populate = populate_completion;
            let back_button = back.downgrade();
            let forward_button = forward.downgrade();
            let address = address.downgrade();
            let last_requested = navigation.last_requested.clone();
            let committed_uri_handlers = committed_uri_handlers.clone();
            web_view.connect_load_changed(move |wv, event| {
                if let (Some(back_button), Some(forward_button)) =
                    (back_button.upgrade(), forward_button.upgrade())
                {
                    sync_history_buttons(wv, &back_button, &forward_button);
                }
                if event == webkit6::LoadEvent::Committed {
                    if let Some(uri) = wv.uri() {
                        let u = uri.to_string();
                        *last_requested.borrow_mut() = u.clone();
                        if let Some(address) = address.upgrade() {
                            if address.text().as_str() != u {
                                address.set_text(&u);
                            }
                        }
                        for handler in committed_uri_handlers.borrow().iter() {
                            handler(u.clone());
                        }
                        if is_recordable_url(&u) {
                            let title = wv.title().map(|t| t.to_string()).unwrap_or_default();
                            if let Ok(store) = forktty_core::HistoryStore::for_profile(profile) {
                                if let Err(e) = store.record_visit(&u, &title) {
                                    eprintln!("forktty: history record_visit failed: {e}");
                                }
                            }
                        }
                    }
                    // Refresh address-bar completion after each committed load.
                    populate(profile);
                }
            });
        }

        // Title-notify handler: update the row once the page <title> is
        // available. WebKit fires LoadEvent::Committed before the title is
        // parsed, so the Committed handler above usually records an empty title.
        // This keeps visit_count tied to committed navigations.
        {
            web_view.connect_title_notify(move |wv| {
                if let Some(uri) = wv.uri() {
                    let u = uri.to_string();
                    if is_recordable_url(&u) {
                        let title = wv.title().map(|t| t.to_string()).unwrap_or_default();
                        // Only bother if we actually have a title now.
                        if !title.is_empty() {
                            if let Ok(store) = forktty_core::HistoryStore::for_profile(profile) {
                                if let Err(e) = store.update_title(&u, &title) {
                                    eprintln!("forktty: history update_title failed: {e}");
                                }
                            }
                        }
                    }
                }
            });
        }

        container.append(&bar);
        container.append(&web_view);

        let widget = Self {
            container,
            toolbar: bar,
            web_view,
            address,
            close,
            // Empty so the first load_uri(initial_url) below is not a no-op;
            // that single call populates last_requested and avoids a double-load.
            last_requested: navigation.last_requested.clone(),
            committed_uri_handlers,
        };
        widget.load_uri(initial_url);
        widget
    }

    pub fn widget(&self) -> gtk::Widget {
        self.container.clone().upcast()
    }

    /// Drag handle used by the pane layout to reorder/swap browser panes.
    pub fn drag_handle(&self) -> gtk::Widget {
        self.toolbar.clone().upcast()
    }

    /// The widget keyboard focus should land on when this pane is focused.
    /// The address entry is the natural keyboard entry point and is always
    /// realized (the WebView may not have loaded yet).
    pub fn focus_target(&self) -> gtk::Widget {
        self.address.clone().upcast()
    }

    pub fn set_active(&self, active: bool) {
        if active {
            self.container.add_css_class("active");
        } else {
            self.container.remove_css_class("active");
        }
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
        BrowserPaneNavigation {
            web_view: self.web_view.clone(),
            address: self.address.clone(),
            last_requested: self.last_requested.clone(),
        }
        .load_uri(url);
    }

    pub fn current_uri(&self) -> Option<String> {
        self.web_view.uri().map(|g| g.to_string())
    }

    pub fn go_back(&self) {
        self.web_view.go_back();
    }

    pub fn go_forward(&self) {
        self.web_view.go_forward();
    }

    pub fn reload(&self) {
        self.web_view.reload();
    }

    /// Connect the address bar's Enter key to a navigation callback.
    pub fn connect_address_activate<F: Fn(String) + 'static>(&self, f: F) {
        let entry = self.address.downgrade();
        self.address.connect_activate(move |_| {
            if let Some(entry) = entry.upgrade() {
                f(entry.text().to_string());
            }
        });
    }

    /// Connect a callback fired after the WebView commits a URI. This includes
    /// redirects, history navigation, and user-driven link clicks.
    pub fn connect_uri_committed<F: Fn(String) + 'static>(&self, f: F) {
        self.committed_uri_handlers.borrow_mut().push(Box::new(f));
    }

    /// Connect the close (×) button to a callback. The widget does not own the
    /// model or the confirmation dialog, so gtk_app wires this to the same
    /// `show_close_pane_confirmation` path terminal panes use.
    pub fn connect_close<F: Fn() + 'static>(&self, f: F) {
        self.close.connect_clicked(move |_| f());
    }

    pub fn prepare_close(&self) {
        self.web_view.stop_loading();
    }

    /// Run JavaScript in the page, delivering the JSON-serialized result (or an
    /// error) to `on_done`. `on_done` runs on the GTK main thread once the GIO
    /// async call settles.
    pub fn run_js<F>(&self, js: &str, on_done: F)
    where
        F: FnOnce(Result<String, BrowserCmdError>) + 'static,
    {
        use webkit6::gio::Cancellable;
        self.web_view
            .evaluate_javascript(js, None, None, Cancellable::NONE, move |result| {
                let mapped = match result {
                    Ok(value) => match value.to_json(0) {
                        Some(s) => {
                            let s = s.to_string();
                            if s.len() >= forktty_core::MAX_BROWSER_RESULT_BYTES {
                                Err(BrowserCmdError::TooLarge)
                            } else {
                                Ok(s)
                            }
                        }
                        None => Ok("null".to_string()),
                    },
                    Err(err) => {
                        let msg = err.to_string();
                        if msg.contains("ref-not-found") {
                            Err(BrowserCmdError::RefNotFound)
                        } else if msg.contains("ref-not-interactable") {
                            Err(BrowserCmdError::ElementNotInteractable)
                        } else {
                            Err(BrowserCmdError::JsError(msg))
                        }
                    }
                };
                on_done(mapped);
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_http_urls_are_recordable() {
        assert!(is_recordable_url("https://a.test/"));
        assert!(is_recordable_url("http://a.test/"));
        assert!(is_recordable_url("HTTPS://a.test/"));
        assert!(is_recordable_url("Http://a.test/"));
        assert!(!is_recordable_url("about:blank"));
        assert!(!is_recordable_url(""));
        assert!(!is_recordable_url("data:text/html,x"));
    }

    #[test]
    fn driver_script_is_present() {
        assert!(DRIVER_JS.contains("window.__forktty"));
        assert!(DRIVER_JS.contains("snapshot"));
        assert!(DRIVER_JS.contains("return root;"));
        assert!(!DRIVER_JS.contains("JSON.stringify(root)"));
        assert!(DRIVER_JS.contains("ref-not-interactable"));
        assert!(DRIVER_JS.contains("isLiveElement"));
    }

    #[test]
    fn click_call_quotes_ref() {
        assert_eq!(click_js("e1"), "window.__forktty.click(\"e1\")");
        assert_eq!(click_js("e\"1"), "window.__forktty.click(\"e\\\"1\")");
    }

    #[test]
    fn fill_call_quotes_ref_and_value() {
        assert_eq!(
            fill_js("e2", "hello"),
            "window.__forktty.fill(\"e2\",\"hello\")"
        );
        assert_eq!(
            fill_js("e2", "a\"b"),
            "window.__forktty.fill(\"e2\",\"a\\\"b\")"
        );
    }

    #[test]
    fn browser_pane_shortcuts_match_common_browser_keys() {
        use gtk::gdk::{Key, ModifierType};

        assert_eq!(
            browser_pane_shortcut(Key::l, ModifierType::CONTROL_MASK),
            Some(BrowserPaneShortcut::FocusAddress)
        );
        assert_eq!(
            browser_pane_shortcut(Key::d, ModifierType::ALT_MASK),
            Some(BrowserPaneShortcut::FocusAddress)
        );
        assert_eq!(
            browser_pane_shortcut(Key::Left, ModifierType::ALT_MASK),
            Some(BrowserPaneShortcut::Back)
        );
        assert_eq!(
            browser_pane_shortcut(Key::Right, ModifierType::ALT_MASK),
            Some(BrowserPaneShortcut::Forward)
        );
        assert_eq!(
            browser_pane_shortcut(Key::r, ModifierType::CONTROL_MASK),
            Some(BrowserPaneShortcut::Reload)
        );
        assert_eq!(
            browser_pane_shortcut(Key::F5, ModifierType::empty()),
            Some(BrowserPaneShortcut::Reload)
        );
        assert_eq!(
            browser_pane_shortcut(Key::Left, ModifierType::empty()),
            None
        );
    }

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
        let pane = BrowserPaneWidget::new(
            crate::browser_session::DEFAULT_PROFILE_ID,
            "https://example.com",
        );
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
