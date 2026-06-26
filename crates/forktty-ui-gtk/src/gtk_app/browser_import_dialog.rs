//! Browser data import dialog and request shaping.

use super::*;

pub(super) fn show_browser_import_dialog(parent: &adw::ApplicationWindow, state: &SocketAppState) {
    let dialog = gtk::Window::builder()
        .title("Import Browser Data")
        .transient_for(parent)
        .modal(true)
        .default_width(620)
        .default_height(560)
        .build();
    dialog.add_css_class("ft-dialog");
    apply_dialog_chrome(&dialog);
    install_escape_close(&dialog);
    restore_focus_after_hide(&dialog, parent);

    let header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    header.add_css_class("ft-dialog-header");
    let title = gtk::Label::builder()
        .label("Import Browser Data")
        .xalign(0.0)
        .build();
    title.add_css_class("ft-dialog-title");
    let subtitle = gtk::Label::builder()
        .label("Select discovered browser profiles, preview counts, then import into a ForkTTY browser profile.")
        .xalign(0.0)
        .wrap(true)
        .build();
    subtitle.add_css_class("ft-dialog-subtitle");
    header.append(&title);
    header.append(&subtitle);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.add_css_class("ft-dialog-body");

    let source_title = gtk::Label::builder()
        .label("Source profiles")
        .xalign(0.0)
        .build();
    source_title.add_css_class("ft-section-title");
    let source_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let loading = gtk::Label::builder()
        .label("Searching local browser profiles...")
        .xalign(0.0)
        .build();
    loading.add_css_class("ft-form-hint");
    source_box.append(&loading);
    let source_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(150)
        .vexpand(true)
        .child(&source_box)
        .build();

    let include_title = gtk::Label::builder().label("Data").xalign(0.0).build();
    include_title.add_css_class("ft-section-title");
    let include_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let include_history = gtk::CheckButton::with_label("History");
    include_history.set_active(true);
    let include_bookmarks = gtk::CheckButton::with_label("Bookmarks");
    include_bookmarks.set_active(true);
    let include_cookies = gtk::CheckButton::with_label("Cookies");
    include_cookies.set_active(true);
    include_cookies.set_tooltip_text(Some(
        "Cookies can be read for preview but are not written yet.",
    ));
    include_box.append(&include_history);
    include_box.append(&include_bookmarks);
    include_box.append(&include_cookies);

    let destination_title = gtk::Label::builder()
        .label("Destination")
        .xalign(0.0)
        .build();
    destination_title.add_css_class("ft-section-title");
    let destination_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let destination = gtk::ComboBoxText::new();
    destination.set_hexpand(true);
    destination.update_property(&[gtk::accessible::Property::Label(
        "Destination ForkTTY browser profile",
    )]);
    let new_profile_name = gtk::Entry::builder()
        .placeholder_text("New profile name")
        .text("Imported Browser")
        .visible(false)
        .build();
    new_profile_name.update_property(&[gtk::accessible::Property::Label("New profile name")]);
    destination_box.append(&destination);
    destination_box.append(&new_profile_name);

    let status = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .build();
    status.add_css_class("ft-inline-status");

    body.append(&source_title);
    body.append(&source_scroll);
    body.append(&include_title);
    body.append(&include_box);
    body.append(&destination_title);
    body.append(&destination_box);
    body.append(&status);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("ft-dialog-footer");
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let cancel = gtk::Button::with_label("Cancel");
    let preview = gtk::Button::with_label("Preview");
    let import = gtk::Button::with_label("Import");
    import.add_css_class("suggested-action");
    preview.set_sensitive(false);
    import.set_sensitive(false);
    footer.append(&spacer);
    footer.append(&cancel);
    footer.append(&preview);
    footer.append(&import);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&header);
    root.append(&body);
    root.append(&footer);
    dialog.set_default_widget(Some(&preview));
    dialog.set_child(Some(&root));

    let checks: Rc<RefCell<Vec<(String, gtk::CheckButton)>>> = Rc::new(RefCell::new(Vec::new()));

    {
        let dialog_for_cancel = dialog.clone();
        cancel.connect_clicked(move |_| dialog_for_cancel.close());
    }

    {
        let new_profile_name = new_profile_name.clone();
        destination.connect_changed(move |combo| {
            let is_new = combo
                .active_id()
                .as_deref()
                .is_some_and(|id| id == "__new__");
            new_profile_name.set_visible(is_new);
        });
    }

    {
        let state = state.clone();
        let source_box = source_box.clone();
        let checks = checks.clone();
        let destination = destination.clone();
        let preview = preview.clone();
        let import = import.clone();
        let status = status.clone();
        glib::spawn_future_local(async move {
            let discovered =
                forktty_socket::dispatch(&state, "browser.import.discover", json!({})).await;
            let profiles =
                forktty_socket::dispatch(&state, "browser.profile.list", json!({})).await;

            while let Some(child) = source_box.first_child() {
                source_box.remove(&child);
            }
            checks.borrow_mut().clear();

            match discovered {
                Ok(value) => {
                    let rows = browser_import_source_rows(&value);
                    if rows.is_empty() {
                        let empty = gtk::Label::builder()
                            .label("No importable browser profiles found.")
                            .xalign(0.0)
                            .wrap(true)
                            .build();
                        empty.add_css_class("ft-form-hint");
                        source_box.append(&empty);
                    } else {
                        for row in rows {
                            let check = gtk::CheckButton::with_label(&row.label);
                            check.set_active(true);
                            check.set_tooltip_text(row.tooltip.as_deref());
                            source_box.append(&check);
                            checks.borrow_mut().push((row.id, check));
                        }
                        preview.set_sensitive(true);
                        import.set_sensitive(true);
                        set_status_message(
                            &status,
                            "Sources loaded. Preview before importing.",
                            StatusKind::Success,
                        );
                    }
                }
                Err(err) => set_status_message(&status, &err.to_string(), StatusKind::Error),
            }

            destination.remove_all();
            if let Ok(value) = profiles {
                if let Some(items) = value.as_array() {
                    for profile in items {
                        if let (Some(id), Some(name)) = (
                            profile.get("id").and_then(Value::as_str),
                            profile.get("display_name").and_then(Value::as_str),
                        ) {
                            destination.append(Some(id), name);
                        }
                    }
                }
            }
            destination.append(Some("__new__"), "New ForkTTY Profile");
            destination.set_active(Some(0));
        });
    }

    {
        let state = state.clone();
        let checks = checks.clone();
        let include_history = include_history.clone();
        let include_bookmarks = include_bookmarks.clone();
        let include_cookies = include_cookies.clone();
        let preview_button = preview.clone();
        let import_button = import.clone();
        let status = status.clone();
        preview.connect_clicked(move |_| {
            let params = match browser_import_dialog_params(
                &checks,
                &include_history,
                &include_bookmarks,
                &include_cookies,
                None,
            ) {
                Ok(params) => params,
                Err(err) => {
                    set_status_message(&status, err.message(), StatusKind::Error);
                    return;
                }
            };
            preview_button.set_sensitive(false);
            import_button.set_sensitive(false);
            set_status_message(&status, "Reading selected sources...", StatusKind::Success);
            let state = state.clone();
            let status = status.clone();
            let preview_button = preview_button.clone();
            let import_button = import_button.clone();
            glib::spawn_future_local(async move {
                match forktty_socket::dispatch(&state, "browser.import.preview", params).await {
                    Ok(value) => set_status_message(
                        &status,
                        &browser_import_preview_summary(&value),
                        StatusKind::Success,
                    ),
                    Err(err) => set_status_message(&status, &err.to_string(), StatusKind::Error),
                }
                preview_button.set_sensitive(true);
                import_button.set_sensitive(true);
            });
        });
    }

    {
        let state = state.clone();
        let checks = checks.clone();
        let include_history = include_history.clone();
        let include_bookmarks = include_bookmarks.clone();
        let include_cookies = include_cookies.clone();
        let destination = destination.clone();
        let new_profile_name = new_profile_name.clone();
        let preview_button = preview.clone();
        let import_button = import.clone();
        let status = status.clone();
        import.connect_clicked(move |_| {
            let active_id = destination.active_id().map(|id| id.to_string());
            let destination = if active_id.as_deref() == Some("__new__") {
                let name = new_profile_name.text().trim().to_string();
                if name.is_empty() {
                    set_status_message(
                        &status,
                        "New profile name cannot be empty.",
                        StatusKind::Error,
                    );
                    return;
                }
                Some(json!({"kind": "create", "display_name": name}))
            } else {
                active_id.map(|id| json!({"kind": "existing", "profile": id}))
            };
            let params = match browser_import_dialog_params(
                &checks,
                &include_history,
                &include_bookmarks,
                &include_cookies,
                destination,
            ) {
                Ok(params) => params,
                Err(err) => {
                    set_status_message(&status, err.message(), StatusKind::Error);
                    return;
                }
            };
            preview_button.set_sensitive(false);
            import_button.set_sensitive(false);
            set_status_message(&status, "Importing selected data...", StatusKind::Success);
            let state = state.clone();
            let status = status.clone();
            let preview_button = preview_button.clone();
            let import_button = import_button.clone();
            glib::spawn_future_local(async move {
                match forktty_socket::dispatch(&state, "browser.import.run", params).await {
                    Ok(value) => set_status_message(
                        &status,
                        &browser_import_run_summary(&value),
                        StatusKind::Success,
                    ),
                    Err(err) => set_status_message(&status, &err.to_string(), StatusKind::Error),
                }
                preview_button.set_sensitive(true);
                import_button.set_sensitive(true);
            });
        });
    }

    dialog.present();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BrowserImportDialogParamError {
    NoSources,
    NoData,
}

impl BrowserImportDialogParamError {
    fn message(self) -> &'static str {
        match self {
            Self::NoSources => "Select at least one source profile.",
            Self::NoData => "Select at least one data type to import.",
        }
    }
}

pub(super) struct BrowserImportSourceRow {
    id: String,
    label: String,
    tooltip: Option<String>,
}

pub(super) fn browser_import_source_rows(discovered: &Value) -> Vec<BrowserImportSourceRow> {
    let mut rows = Vec::new();
    let Some(browsers) = discovered.get("browsers").and_then(Value::as_array) else {
        return rows;
    };
    for browser in browsers {
        let label = browser
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or("Browser");
        let Some(profiles) = browser.get("profiles").and_then(Value::as_array) else {
            continue;
        };
        for profile in profiles {
            let Some(id) = profile.get("id").and_then(Value::as_str) else {
                continue;
            };
            let name = profile
                .get("display_name")
                .and_then(Value::as_str)
                .unwrap_or("Profile");
            let path = profile.get("path").and_then(Value::as_str);
            rows.push(BrowserImportSourceRow {
                id: id.to_string(),
                label: format!("{label} - {name}"),
                tooltip: path.map(str::to_string),
            });
        }
    }
    rows
}

pub(super) fn browser_import_dialog_params(
    checks: &Rc<RefCell<Vec<(String, gtk::CheckButton)>>>,
    include_history: &gtk::CheckButton,
    include_bookmarks: &gtk::CheckButton,
    include_cookies: &gtk::CheckButton,
    destination: Option<Value>,
) -> Result<Value, BrowserImportDialogParamError> {
    let sources: Vec<Value> = checks
        .borrow()
        .iter()
        .filter(|(_, check)| check.is_active())
        .map(|(id, _)| Value::String(id.clone()))
        .collect();
    browser_import_dialog_params_from_parts(
        sources,
        include_history.is_active(),
        include_bookmarks.is_active(),
        include_cookies.is_active(),
        destination,
    )
}

pub(super) fn browser_import_dialog_params_from_parts(
    sources: Vec<Value>,
    include_history: bool,
    include_bookmarks: bool,
    include_cookies: bool,
    destination: Option<Value>,
) -> Result<Value, BrowserImportDialogParamError> {
    if sources.is_empty() {
        return Err(BrowserImportDialogParamError::NoSources);
    }
    if !(include_history || include_bookmarks || include_cookies) {
        return Err(BrowserImportDialogParamError::NoData);
    }
    let mut params = json!({
        "sources": sources,
        "include": {
            "history": include_history,
            "bookmarks": include_bookmarks,
            "cookies": include_cookies,
        }
    });
    if let Some(destination) = destination {
        params["destination"] = destination;
    }
    Ok(params)
}

pub(super) fn browser_import_count(value: &Value, key: &str) -> usize {
    value.get(key).and_then(Value::as_u64).unwrap_or(0) as usize
}

pub(super) fn browser_import_preview_summary(value: &Value) -> String {
    let total = value.get("total").unwrap_or(&Value::Null);
    let history = browser_import_count(total, "history");
    let bookmarks = browser_import_count(total, "bookmarks");
    let cookies = browser_import_count(total, "cookies");
    let skipped = browser_import_count(total, "skipped");
    format!(
        "Preview: {history} history rows, {bookmarks} bookmarks, {cookies} cookies read, {skipped} skipped. Cookies are not written yet."
    )
}

pub(super) fn browser_import_run_summary(value: &Value) -> String {
    let total = value.get("total").unwrap_or(&Value::Null);
    let written = total.get("written").unwrap_or(&Value::Null);
    let cookies = total.get("cookies").unwrap_or(&Value::Null);
    let history = browser_import_count(written, "history");
    let bookmarks = browser_import_count(written, "bookmarks");
    let cookies_read = browser_import_count(cookies, "read");
    let unsupported = browser_import_count(cookies, "unsupported");
    let skipped = browser_import_count(cookies, "skipped");
    format!(
        "Imported {history} history rows and {bookmarks} bookmarks. Cookies read: {cookies_read}; unsupported for writing: {unsupported}; skipped: {skipped}."
    )
}
