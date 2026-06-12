use super::*;

#[derive(Clone)]
pub(super) struct ToastHandle {
    overlay: glib::WeakRef<adw::ToastOverlay>,
}

impl std::fmt::Debug for ToastHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ToastHandle")
    }
}

impl ToastHandle {
    pub(super) fn new(overlay: &adw::ToastOverlay) -> Self {
        Self {
            overlay: overlay.downgrade(),
        }
    }

    pub(super) fn show(&self, message: &str) {
        if let Some(overlay) = self.overlay.upgrade() {
            overlay.add_toast(adw::Toast::new(message));
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum StatusKind {
    Success,
    Error,
}

macro_rules! define_internal_dnd_payload {
    ($payload:ident, $type_name:literal, $source_fn:ident, $target_fn:ident, $type_fn:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, glib::Boxed)]
        #[boxed_type(name = $type_name)]
        pub(super) struct $payload(String);

        impl $payload {
            fn new(id: &str) -> Self {
                Self(id.to_string())
            }

            fn id(&self) -> String {
                self.0.clone()
            }
        }

        pub(super) fn $source_fn<W>(widget: &W, id: &str)
        where
            W: IsA<gtk::Widget>,
        {
            install_internal_drag_source(widget, $payload::new(id).to_value());
        }

        pub(super) fn $target_fn<F>(on_drop: F) -> gtk::DropTarget
        where
            F: Fn(String, f64, f64) -> bool + 'static,
        {
            let target = gtk::DropTarget::new($payload::static_type(), gdk::DragAction::MOVE);
            target.connect_drop(move |_, value, x, y| {
                let Ok(payload) = value.get::<$payload>() else {
                    return false;
                };
                on_drop(payload.id(), x, y)
            });
            target
        }

        #[cfg(test)]
        pub(super) fn $type_fn() -> glib::Type {
            $payload::static_type()
        }
    };
}

define_internal_dnd_payload!(
    WorkspaceDndPayload,
    "ForkttyWorkspaceDndPayload",
    install_workspace_drag_source,
    workspace_drop_target,
    workspace_dnd_type
);
define_internal_dnd_payload!(
    TabDndPayload,
    "ForkttyTabDndPayload",
    install_tab_drag_source,
    tab_drop_target,
    tab_dnd_type
);
define_internal_dnd_payload!(
    PaneDndPayload,
    "ForkttyPaneDndPayload",
    install_pane_drag_source,
    pane_drop_target,
    pane_dnd_type
);

pub(super) fn tab_dnd_id_from_value(value: &glib::Value) -> Option<String> {
    value
        .get::<TabDndPayload>()
        .ok()
        .map(|payload| payload.id())
}

pub(super) fn workspace_dnd_id_from_value(value: &glib::Value) -> Option<String> {
    value
        .get::<WorkspaceDndPayload>()
        .ok()
        .map(|payload| payload.id())
}

pub(super) fn pane_dnd_id_from_value(value: &glib::Value) -> Option<String> {
    value
        .get::<PaneDndPayload>()
        .ok()
        .map(|payload| payload.id())
}

#[cfg(test)]
pub(super) fn tab_dnd_value(id: &str) -> glib::Value {
    TabDndPayload::new(id).to_value()
}

fn install_internal_drag_source<W>(widget: &W, payload: glib::Value)
where
    W: IsA<gtk::Widget>,
{
    static TRANSPARENT_DRAG_PIXEL: [u8; 4] = [0, 0, 0, 0];

    let source = gtk::DragSource::builder()
        .actions(gdk::DragAction::MOVE)
        .build();
    // Capture avoids child buttons/list rows claiming the pointer sequence
    // before GtkDragSource can recognize a drag.
    source.set_propagation_phase(gtk::PropagationPhase::Capture);
    source.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&payload)));
    source.connect_drag_begin(move |source, _| {
        let bytes = glib::Bytes::from_static(&TRANSPARENT_DRAG_PIXEL);
        let icon = gdk::MemoryTexture::new(1, 1, gdk::MemoryFormat::R8g8b8a8, &bytes, 4);
        source.set_icon(Some(&icon), 0, 0);
    });
    widget.add_controller(source);
}

pub(super) fn drop_position(coord: f64, span: i32) -> forktty_core::MovePosition {
    if coord < f64::from(span.max(1)) / 2.0 {
        forktty_core::MovePosition::Before
    } else {
        forktty_core::MovePosition::After
    }
}

pub(super) fn labeled_icon_button_parts(
    icon_name: &str,
    label: &str,
) -> (gtk::Button, gtk::Image, gtk::Label) {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.set_halign(gtk::Align::Center);
    let icon = gtk::Image::from_icon_name(icon_name);
    let label_widget = gtk::Label::new(Some(label));
    content.append(&icon);
    content.append(&label_widget);
    let button = gtk::Button::builder().child(&content).build();
    set_accessible_button_text(&button, label, None);
    (button, icon, label_widget)
}

pub(super) fn apply_dialog_chrome(dialog: &gtk::Window) {
    let titlebar = gtk::HeaderBar::new();
    titlebar.set_show_title_buttons(false);
    titlebar.add_css_class("ft-dialog-titlebar");
    titlebar.set_title_widget(Some(&gtk::Label::new(None)));
    let close = gtk::Button::builder()
        .icon_name("forktty-close-symbolic")
        .tooltip_text("Close")
        .build();
    close.add_css_class("flat");
    close.add_css_class("ft-dialog-close");
    set_accessible_button_text(&close, "Close", Some("Esc"));
    let dialog_for_close = dialog.clone();
    close.connect_clicked(move |_| dialog_for_close.close());
    titlebar.pack_end(&close);
    dialog.set_titlebar(Some(&titlebar));
}

pub(super) fn restore_focus_after_hide<W>(dialog: &gtk::Window, parent: &W)
where
    W: IsA<gtk::Window>,
{
    let previous_focus: Option<gtk::Widget> = gtk::prelude::GtkWindowExt::focus(parent.as_ref());
    dialog.connect_hide(move |_| {
        if let Some(widget) = previous_focus.as_ref() {
            if widget.root().is_some() {
                widget.grab_focus();
            }
        }
    });
}

pub(super) fn install_escape_close(window: &gtk::Window) {
    let controller = gtk::EventControllerKey::new();
    let window_for_close = window.clone();
    controller.connect_key_pressed(move |_, key, _, modifiers| {
        let is_close_shortcut =
            key == gtk::gdk::Key::w && modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
        if key == gtk::gdk::Key::Escape || is_close_shortcut {
            window_for_close.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(controller);
}

const FORKTTY_SUPPORT_URI: &str = "https://ko-fi.com/lucenx9";
const FORKTTY_REPOSITORY_URI: &str = env!("CARGO_PKG_REPOSITORY");
const FORKTTY_LICENSE: &str = "AGPL-3.0-only";

pub(super) fn open_support_uri() {
    open_external_uri(FORKTTY_SUPPORT_URI, "support page");
}

pub(super) fn show_about_dialog(parent: &adw::ApplicationWindow) {
    let dialog = gtk::Window::builder()
        .title("About ForkTTY")
        .transient_for(parent)
        .modal(true)
        .default_width(420)
        .default_height(430)
        .resizable(false)
        .build();
    dialog.add_css_class("ft-dialog");
    dialog.add_css_class("about-dialog");
    apply_dialog_chrome(&dialog);
    install_escape_close(&dialog);
    restore_focus_after_hide(&dialog, parent);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("about-body");

    let hero = gtk::Box::new(gtk::Orientation::Vertical, 7);
    hero.add_css_class("about-hero");
    let logo = gtk::Image::from_icon_name("forktty");
    logo.set_pixel_size(96);
    logo.add_css_class("about-logo");
    let name = gtk::Label::builder().label("ForkTTY").build();
    name.add_css_class("about-name");
    let version = gtk::Label::builder()
        .label(format!("Version {}", env!("CARGO_PKG_VERSION")))
        .build();
    version.add_css_class("about-version");
    let description = gtk::Label::builder()
        .label("A native GTK/Ghostty terminal for panes, worktrees, and automation.")
        .wrap(true)
        .justify(gtk::Justification::Center)
        .max_width_chars(44)
        .build();
    description.add_css_class("about-description");
    hero.append(&logo);
    hero.append(&name);
    hero.append(&version);
    hero.append(&description);
    body.append(&hero);

    let details = gtk::Box::new(gtk::Orientation::Vertical, 0);
    details.add_css_class("about-details");
    details.append(&about_detail_row("Developer", "Lucenx9"));
    details.append(&about_detail_row("License", FORKTTY_LICENSE));
    details.append(&about_detail_row("Copyright", "2026 Lucenx9"));
    body.append(&details);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.add_css_class("about-actions");
    actions.set_halign(gtk::Align::Center);
    let repository = about_action_button("Source Code");
    repository.connect_clicked(move |_| {
        open_external_uri(FORKTTY_REPOSITORY_URI, "repository");
    });
    let support = about_action_button("Support me");
    support.add_css_class("suggested-action");
    support.connect_clicked(move |_| open_support_uri());
    actions.append(&repository);
    actions.append(&support);
    body.append(&actions);

    dialog.set_child(Some(&body));
    dialog.present();
}

fn open_external_uri(uri: &str, description: &str) {
    if let Err(err) = gio::AppInfo::launch_default_for_uri(uri, None::<&gio::AppLaunchContext>) {
        eprintln!("Could not open {description}: {err}");
    }
}

fn about_detail_row(label: &str, value: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("about-detail-row");
    let label = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .width_request(92)
        .build();
    label.add_css_class("about-detail-label");
    let value = gtk::Label::builder()
        .label(value)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    value.add_css_class("about-detail-value");
    row.append(&label);
    row.append(&value);
    row
}

fn about_action_button(label: &str) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    button.add_css_class("about-action");
    button
}

pub(super) fn show_destructive_confirmation<W, F>(
    parent: &W,
    title: &str,
    body: &str,
    confirm_label: &str,
    on_confirm: F,
) where
    W: IsA<gtk::Window>,
    F: Fn() + 'static,
{
    let dialog = gtk::Window::builder()
        .title(title)
        .transient_for(parent)
        .modal(true)
        .default_width(400)
        .default_height(200)
        .build();
    dialog.add_css_class("ft-dialog");
    apply_dialog_chrome(&dialog);
    install_escape_close(&dialog);
    restore_focus_after_hide(&dialog, parent);

    let header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    header.add_css_class("ft-dialog-header");
    let title_label = gtk::Label::builder().label(title).xalign(0.0).build();
    title_label.add_css_class("ft-dialog-title");
    header.append(&title_label);

    let body_container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body_container.add_css_class("ft-dialog-body");
    let body_label = gtk::Label::builder()
        .label(body)
        .xalign(0.0)
        .wrap(true)
        .build();
    body_label.add_css_class("ft-dialog-confirm-body");
    body_container.append(&body_label);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("ft-dialog-footer");
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let cancel = gtk::Button::with_label("Cancel");
    let confirm = gtk::Button::with_label(confirm_label);
    confirm.add_css_class("destructive-action");
    footer.append(&spacer);
    footer.append(&cancel);
    footer.append(&confirm);

    let dialog_for_cancel = dialog.clone();
    cancel.connect_clicked(move |_| dialog_for_cancel.close());
    let dialog_for_confirm = dialog.clone();
    confirm.connect_clicked(move |_| {
        on_confirm();
        dialog_for_confirm.close();
    });

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&header);
    content.append(&body_container);
    content.append(&footer);
    dialog.set_default_widget(Some(&cancel));
    dialog.set_child(Some(&content));
    dialog.present();
    cancel.grab_focus();
}

pub(super) fn show_rename_workspace_dialog<W>(
    parent: &W,
    state: &SocketAppState,
    workspace_id: &str,
    current_name: &str,
) where
    W: IsA<gtk::Window>,
{
    let dialog = gtk::Window::builder()
        .title("Rename Workspace")
        .transient_for(parent)
        .modal(true)
        .default_width(420)
        .default_height(190)
        .build();
    dialog.add_css_class("ft-dialog");
    apply_dialog_chrome(&dialog);
    install_escape_close(&dialog);
    restore_focus_after_hide(&dialog, parent);

    let header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    header.add_css_class("ft-dialog-header");
    let title = gtk::Label::builder()
        .label("Rename Workspace")
        .xalign(0.0)
        .build();
    title.add_css_class("ft-dialog-title");
    let subtitle = gtk::Label::builder()
        .label("Choose a short name that is easy to recognize in the sidebar and status bar.")
        .xalign(0.0)
        .wrap(true)
        .build();
    subtitle.add_css_class("ft-dialog-subtitle");
    header.append(&title);
    header.append(&subtitle);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.add_css_class("ft-dialog-body");
    let entry = gtk::Entry::builder()
        .text(current_name)
        .placeholder_text("Workspace name")
        .hexpand(true)
        .build();
    entry.update_property(&[gtk::accessible::Property::Label("Workspace name")]);
    let status = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .build();
    status.add_css_class("ft-inline-status");
    body.append(&entry);
    body.append(&status);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("ft-dialog-footer");
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let cancel = gtk::Button::with_label("Cancel");
    let rename = gtk::Button::with_label("Rename");
    rename.add_css_class("suggested-action");
    rename.set_sensitive(false);
    footer.append(&spacer);
    footer.append(&cancel);
    footer.append(&rename);

    let current_name_owned = current_name.to_string();
    let status_for_change = status.clone();
    let rename_for_change = rename.clone();
    entry.connect_changed(move |entry| {
        let candidate = entry.text();
        let trimmed = candidate.trim();
        let valid = !trimmed.is_empty() && trimmed != current_name_owned;
        rename_for_change.set_sensitive(valid);
        if trimmed.is_empty() {
            set_status_message(
                &status_for_change,
                "Workspace name cannot be empty.",
                StatusKind::Error,
            );
        } else {
            clear_status_message(&status_for_change);
        }
    });

    let dialog_for_cancel = dialog.clone();
    cancel.connect_clicked(move |_| dialog_for_cancel.close());

    let state_for_rename = state.clone();
    let workspace_id_for_rename = workspace_id.to_string();
    let dialog_for_rename = dialog.clone();
    let status_for_rename = status.clone();
    let entry_for_rename = entry.clone();
    rename.connect_clicked(move |_| {
        match rename_workspace_gtk(
            &state_for_rename,
            &workspace_id_for_rename,
            entry_for_rename.text().as_str(),
        ) {
            Ok(()) => dialog_for_rename.close(),
            Err(err) => set_status_message(&status_for_rename, &err, StatusKind::Error),
        }
    });

    let rename_for_activate = rename.clone();
    entry.connect_activate(move |_| {
        if rename_for_activate.is_sensitive() {
            rename_for_activate.emit_clicked();
        }
    });

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&header);
    content.append(&body);
    content.append(&footer);
    dialog.set_default_widget(Some(&rename));
    dialog.set_child(Some(&content));
    dialog.present();
    entry.grab_focus();
    entry.select_region(0, -1);
}

pub(super) fn set_accessible_button_text(
    button: &gtk::Button,
    label: &str,
    shortcut: Option<&str>,
) {
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

pub(super) fn accessible_shortcut_text(shortcut: &str) -> String {
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

pub(super) fn set_status_message(label: &gtk::Label, message: &str, kind: StatusKind) {
    label.set_text(message);
    label.set_visible(!message.is_empty());
    label.remove_css_class("success");
    label.remove_css_class("error");
    match kind {
        StatusKind::Success => label.add_css_class("success"),
        StatusKind::Error => label.add_css_class("error"),
    }
}

pub(super) fn clear_status_message(label: &gtk::Label) {
    label.set_text("");
    label.set_visible(false);
    label.remove_css_class("success");
    label.remove_css_class("error");
}

pub(super) fn refresh_notification_indicator(button: &gtk::Button, state: &SocketAppState) {
    let badge = button
        .child()
        .and_then(|child| child.downcast::<gtk::Overlay>().ok())
        .and_then(|overlay| overlay.first_child())
        .and_then(|first| first.next_sibling())
        .and_then(|child| child.downcast::<gtk::Label>().ok());

    let (total, unread) = state
        .model
        .lock()
        .ok()
        .map(|model| {
            (
                model.list_notifications().len(),
                model.unread_notification_count(),
            )
        })
        .unwrap_or((0, 0));
    if unread == 0 {
        button.remove_css_class("needs-attention");
        if let Some(badge) = &badge {
            badge.set_visible(false);
        }
        let label = if total == 0 {
            "Notifications".to_string()
        } else if total == 1 {
            "Notifications: 1 read".to_string()
        } else {
            format!("Notifications: {total} read")
        };
        button.set_tooltip_text(Some(&format!("{label} (Ctrl+Shift+M)")));
        set_accessible_button_text(button, &label, Some("Ctrl+Shift+M"));
    } else {
        button.add_css_class("needs-attention");
        if let Some(badge) = &badge {
            let display = if unread > 99 {
                "99+".to_string()
            } else {
                unread.to_string()
            };
            badge.set_text(&display);
            badge.set_visible(true);
        }
        let label = if unread == 1 {
            "Notifications: 1 unread".to_string()
        } else {
            format!("Notifications: {unread} unread")
        };
        button.set_tooltip_text(Some(&format!("{label} (Ctrl+Shift+M)")));
        set_accessible_button_text(button, &label, Some("Ctrl+Shift+M"));
    }
}

pub(super) fn notification_kind_label(kind: NotificationKind) -> &'static str {
    match kind {
        NotificationKind::Prompt => "Prompt",
        NotificationKind::Error => "Error",
        NotificationKind::Info => "Info",
        NotificationKind::Custom => "Custom",
    }
}

pub(super) fn notification_kind_class(kind: NotificationKind) -> &'static str {
    match kind {
        NotificationKind::Prompt => "prompt",
        NotificationKind::Error => "error",
        NotificationKind::Info => "info",
        NotificationKind::Custom => "custom",
    }
}

pub(super) fn notification_age_label(created_at_ms: u128) -> String {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let elapsed = now_ms.saturating_sub(created_at_ms);
    let seconds = elapsed / 1000;
    if seconds < 60 {
        "Just now".to_string()
    } else if seconds < 60 * 60 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 24 * 60 * 60 {
        format!("{}h ago", seconds / 3600)
    } else {
        format!("{}d ago", seconds / 86_400)
    }
}

pub(super) fn add_context_menu_item<F>(
    menu: &gtk::Box,
    popover: &gtk::Popover,
    icon_name: &str,
    label: &str,
    destructive: bool,
    action: F,
) where
    F: Fn() + 'static,
{
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    content.set_hexpand(true);
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.add_css_class("ft-menu-icon");
    let label_widget = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    label_widget.add_css_class("ft-menu-label");
    content.append(&icon);
    content.append(&label_widget);

    let button = gtk::Button::builder().child(&content).build();
    button.add_css_class("flat");
    button.add_css_class("ft-menu-item");
    if destructive {
        button.add_css_class("destructive-action");
    }
    button.set_halign(gtk::Align::Fill);
    let popover = popover.downgrade();
    button.connect_clicked(move |_| {
        action();
        if let Some(popover) = popover.upgrade() {
            popover.popdown();
        }
    });
    menu.append(&button);
}

pub(super) fn add_context_menu_header(menu: &gtk::Box, workspace: &forktty_core::Workspace) {
    let header = gtk::Box::new(gtk::Orientation::Vertical, 1);
    header.add_css_class("ft-menu-header");

    let title = gtk::Label::builder()
        .label(&workspace.name)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    title.add_css_class("ft-menu-header-title");
    let subtitle = gtk::Label::builder()
        .label(compact_path(&workspace.working_dir))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    subtitle.add_css_class("ft-menu-header-subtitle");

    header.append(&title);
    header.append(&subtitle);
    menu.append(&header);
}

pub(super) fn add_context_menu_separator(menu: &gtk::Box) {
    let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
    sep.add_css_class("ft-menu-separator");
    menu.append(&sep);
}

pub(super) fn copy_to_clipboard(text: &str) {
    if let Some(display) = gtk::gdk::Display::default() {
        display.clipboard().set_text(text);
    }
}
