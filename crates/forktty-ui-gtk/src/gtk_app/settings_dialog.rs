use super::*;

pub(super) fn show_settings_dialog(
    parent: &adw::ApplicationWindow,
    state: &SocketAppState,
    on_apply: SettingsApplyCallback,
) {
    #[cfg(not(feature = "browser"))]
    let _ = state;

    let dialog = adw::PreferencesWindow::builder()
        .title("Settings")
        .transient_for(parent)
        .modal(true)
        .default_width(840)
        .default_height(680)
        .search_enabled(true)
        .build();
    let loaded = config::load_config().unwrap_or_default();
    let current = Rc::new(RefCell::new(loaded.clone()));
    let suppress_updates = Rc::new(Cell::new(false));

    install_escape_close(dialog.upcast_ref::<gtk::Window>());
    restore_focus_after_hide(dialog.upcast_ref::<gtk::Window>(), parent);

    let terminal_page = adw::PreferencesPage::builder()
        .title("Terminal")
        .icon_name("utilities-terminal-symbolic")
        .build();
    let shell_group = adw::PreferencesGroup::builder()
        .title("Shell")
        .description("Controls how new terminal sessions are started.")
        .build();
    let shell_entry = adw::EntryRow::builder()
        .title("Shell command")
        .text(&loaded.general.shell)
        .show_apply_button(true)
        .tooltip_text("Absolute path to the shell executable")
        .build();
    shell_group.add(&shell_entry);
    terminal_page.add(&shell_group);

    let font_group = adw::PreferencesGroup::builder()
        .title("Text")
        .description("Applied immediately to all open VTE panes.")
        .build();
    let font_family = font_family_combo(parent, &loaded.appearance.font_family);
    font_family.set_tooltip_text(Some("Terminal font family"));
    font_family.set_hexpand(false);
    font_family.set_halign(gtk::Align::End);
    font_family.set_valign(gtk::Align::Center);
    font_family.set_width_request(220);
    for cell in font_family.cells() {
        if let Ok(text) = cell.downcast::<gtk::CellRendererText>() {
            text.set_ellipsize(gtk::pango::EllipsizeMode::End);
            text.set_max_width_chars(28);
        }
    }
    let font_family_row =
        settings_action_row("Font family", "Monospace font with symbol coverage.");
    font_family_row.add_suffix(&font_family);
    font_family_row.set_activatable_widget(Some(&font_family));
    font_group.add(&font_family_row);

    let (font_size_row, font_size) = settings_number_row(
        "Font size",
        "Terminal text size, in points.",
        8,
        64,
        1,
        i64::from(loaded.appearance.font_size),
        96,
    );
    font_group.add(&font_size_row);
    terminal_page.add(&font_group);

    let behavior_group = adw::PreferencesGroup::builder()
        .title("Behavior")
        .description("Runtime behavior for VTE panes.")
        .build();
    let (scrollback_lines_row, scrollback_lines) = settings_number_row(
        "Scrollback lines",
        "Set to 0 to disable saved scrollback for each pane.",
        0,
        500_000,
        1000,
        i64::from(loaded.appearance.scrollback_lines),
        128,
    );
    behavior_group.add(&scrollback_lines_row);
    let terminal_audible_bell = adw::SwitchRow::builder()
        .title("Audible bell")
        .subtitle("Let terminal bell sequences play the system alert sound.")
        .active(loaded.appearance.terminal_audible_bell)
        .build();
    behavior_group.add(&terminal_audible_bell);
    terminal_page.add(&behavior_group);
    dialog.add(&terminal_page);

    let appearance_page = adw::PreferencesPage::builder()
        .title("Interface")
        .icon_name("preferences-desktop-theme-symbolic")
        .build();
    let theme_group = adw::PreferencesGroup::builder()
        .title("Theme")
        .description("ForkTTY uses a dark-only interface; terminal palettes remain configurable.")
        .build();
    let (terminal_theme_row, terminal_theme) = settings_combo_row(
        "Terminal theme",
        "System uses ForkTTY's neutral dark palette; named themes use fixed dark palettes.",
        TERMINAL_THEME_ITEMS,
        &loaded.appearance.terminal_theme,
    );
    theme_group.add(&terminal_theme_row);
    appearance_page.add(&theme_group);

    let window_group = adw::PreferencesGroup::builder()
        .title("Window")
        .description("Window layout and workspace sidebar behavior.")
        .build();
    let (window_mode_row, window_mode) = settings_combo_row(
        "Window mode",
        "Quake mode uses a drop-down window after restart.",
        &[("normal", "Normal"), ("quake", "Quake")],
        &loaded.appearance.window_mode,
    );
    window_group.add(&window_mode_row);
    let (sidebar_position_row, sidebar_position) = settings_combo_row(
        "Sidebar position",
        "Side of the main window used for workspaces.",
        &[("left", "Left"), ("right", "Right")],
        &loaded.appearance.sidebar_position,
    );
    window_group.add(&sidebar_position_row);
    let sidebar_visible = adw::SwitchRow::builder()
        .title("Show sidebar on startup")
        .subtitle("You can still toggle it with Ctrl+B or F9.")
        .active(loaded.appearance.sidebar_visible)
        .build();
    window_group.add(&sidebar_visible);
    appearance_page.add(&window_group);
    dialog.add(&appearance_page);

    #[cfg(feature = "browser")]
    {
        let browser_page = adw::PreferencesPage::builder()
            .title("Browser")
            .icon_name("web-browser-symbolic")
            .build();
        let import_group = adw::PreferencesGroup::builder()
            .title("Browser Data")
            .description("Import local browser profiles into ForkTTY browser profiles.")
            .build();
        let import_row = settings_action_row(
            "Import Browser Data",
            "Import history and bookmarks from discovered local browser profiles.",
        );
        let import_button = gtk::Button::with_label("Import");
        import_row.add_suffix(&import_button);
        import_row.set_activatable_widget(Some(&import_button));
        import_group.add(&import_row);
        browser_page.add(&import_group);
        dialog.add(&browser_page);

        let parent_for_import = parent.clone();
        let state_for_import = state.clone();
        import_button.connect_clicked(move |_| {
            show_browser_import_dialog(&parent_for_import, &state_for_import);
        });
    }

    let automation_page = adw::PreferencesPage::builder()
        .title("Automation")
        .icon_name("system-run-symbolic")
        .build();
    let worktree_group = adw::PreferencesGroup::builder()
        .title("Git Worktrees")
        .description("Controls where new worktree directories are created.")
        .build();
    let (worktree_layout_row, worktree_layout) = settings_combo_row(
        "Worktree layout",
        "Placement for new worktree directories relative to the repository root.",
        &[
            ("nested", "Nested"),
            ("sibling", "Sibling"),
            ("outer-nested", "Outer nested"),
        ],
        &loaded.general.worktree_layout,
    );
    worktree_group.add(&worktree_layout_row);
    let pr_lookup = adw::SwitchRow::builder()
        .title("Linked PR lookup")
        .subtitle("Use the GitHub CLI to show PR status for workspace branches.")
        .active(loaded.general.enable_pr_lookup)
        .build();
    worktree_group.add(&pr_lookup);
    automation_page.add(&worktree_group);

    let notification_group = adw::PreferencesGroup::builder()
        .title("Notifications")
        .description("In-app notifications always remain available in the notification panel.")
        .build();
    let notification_command = adw::EntryRow::builder()
        .title("Custom command")
        .text(&loaded.general.notification_command)
        .show_apply_button(true)
        .tooltip_text("Optional absolute command to run when a notification fires")
        .build();
    notification_command.set_input_purpose(gtk::InputPurpose::Terminal);
    notification_group.add(&notification_command);
    let desktop_notifications = adw::SwitchRow::builder()
        .title("Desktop notifications")
        .subtitle("Forward alerts to the system notification daemon.")
        .active(loaded.notifications.desktop)
        .build();
    notification_group.add(&desktop_notifications);
    let notification_sound = adw::SwitchRow::builder()
        .title("Alert sound")
        .subtitle("Play the default system alert sound for ForkTTY alerts.")
        .active(loaded.notifications.sound)
        .build();
    notification_group.add(&notification_sound);
    automation_page.add(&notification_group);

    let maintenance_group = adw::PreferencesGroup::builder()
        .title("Advanced")
        .description("Preferences are saved to the user config file immediately.")
        .build();
    let reset_row = settings_action_row(
        "Reset to defaults",
        "Restore the default shell, appearance, workspace and notification preferences.",
    );
    let reset = gtk::Button::with_label("Reset");
    reset.add_css_class("destructive-action");
    reset_row.add_suffix(&reset);
    reset_row.set_activatable_widget(Some(&reset));
    maintenance_group.add(&reset_row);
    automation_page.add(&maintenance_group);
    dialog.add(&automation_page);

    shell_entry.connect_apply({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        move |row: &adw::EntryRow| {
            let mut next = current.borrow().clone();
            next.general.shell = row.text().to_string();
            persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                next,
                "Shell saved. Restart ForkTTY to use it.",
            );
        }
    });
    font_family.connect_changed({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |combo| {
            if suppress_updates.get() {
                return;
            }
            if let Some(family) = combo.active_id() {
                let family = family.to_string();
                let mut next = current.borrow().clone();
                next.appearance.font_family = match family.as_str() {
                    DEFAULT_FONT_FAMILY_ID => String::new(),
                    SYSTEM_MONOSPACE_FONT_FAMILY_ID => "monospace".to_string(),
                    _ => decode_font_family_row_id(&family),
                };
                persist_settings_change(
                    &dialog,
                    &current,
                    &on_apply,
                    next,
                    "Terminal font applied.",
                );
            }
        }
    });
    connect_settings_number_control(&font_size, {
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |value| {
            if suppress_updates.get() {
                return;
            }
            let mut next = current.borrow().clone();
            next.appearance.font_size = value as u16;
            persist_settings_change(&dialog, &current, &on_apply, next, "Font size applied.");
        }
    });
    connect_settings_number_control(&scrollback_lines, {
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |value| {
            if suppress_updates.get() {
                return;
            }
            let mut next = current.borrow().clone();
            next.appearance.scrollback_lines = value as u32;
            persist_settings_change(&dialog, &current, &on_apply, next, "Scrollback updated.");
        }
    });
    terminal_audible_bell.connect_notify_local(Some("active"), {
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |row: &adw::SwitchRow, _| {
            if suppress_updates.get() {
                return;
            }
            let mut next = current.borrow().clone();
            next.appearance.terminal_audible_bell = row.is_active();
            persist_settings_change(&dialog, &current, &on_apply, next, "Terminal bell updated.");
        }
    });
    terminal_theme.connect_changed({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |combo| {
            if suppress_updates.get() {
                return;
            }
            if let Some(theme) = combo.active_id() {
                let mut next = current.borrow().clone();
                next.appearance.terminal_theme = theme.to_string();
                persist_settings_change(
                    &dialog,
                    &current,
                    &on_apply,
                    next,
                    "Terminal theme applied.",
                );
            }
        }
    });
    window_mode.connect_changed({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |combo| {
            if suppress_updates.get() {
                return;
            }
            if let Some(mode) = combo.active_id() {
                let mut next = current.borrow().clone();
                next.appearance.window_mode = mode.to_string();
                persist_settings_change(
                    &dialog,
                    &current,
                    &on_apply,
                    next,
                    "Window mode saved. Restart ForkTTY to use it.",
                );
            }
        }
    });
    sidebar_position.connect_changed({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |combo| {
            if suppress_updates.get() {
                return;
            }
            if let Some(position) = combo.active_id() {
                let mut next = current.borrow().clone();
                next.appearance.sidebar_position = position.to_string();
                persist_settings_change(&dialog, &current, &on_apply, next, "Sidebar moved.");
            }
        }
    });
    sidebar_visible.connect_notify_local(Some("active"), {
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |row: &adw::SwitchRow, _| {
            if suppress_updates.get() {
                return;
            }
            let mut next = current.borrow().clone();
            next.appearance.sidebar_visible = row.is_active();
            persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                next,
                "Sidebar visibility updated.",
            );
        }
    });
    worktree_layout.connect_changed({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |combo| {
            if suppress_updates.get() {
                return;
            }
            if let Some(layout) = combo.active_id() {
                let mut next = current.borrow().clone();
                next.general.worktree_layout = layout.to_string();
                persist_settings_change(
                    &dialog,
                    &current,
                    &on_apply,
                    next,
                    "Worktree layout saved.",
                );
            }
        }
    });
    pr_lookup.connect_notify_local(Some("active"), {
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |row: &adw::SwitchRow, _| {
            if suppress_updates.get() {
                return;
            }
            let mut next = current.borrow().clone();
            next.general.enable_pr_lookup = row.is_active();
            persist_settings_change(&dialog, &current, &on_apply, next, "PR lookup updated.");
        }
    });
    notification_command.connect_apply({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        move |row: &adw::EntryRow| {
            let mut next = current.borrow().clone();
            next.general.notification_command = row.text().to_string();
            persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                next,
                "Notification command saved.",
            );
        }
    });
    desktop_notifications.connect_notify_local(Some("active"), {
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |row: &adw::SwitchRow, _| {
            if suppress_updates.get() {
                return;
            }
            let mut next = current.borrow().clone();
            next.notifications.desktop = row.is_active();
            persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                next,
                "Desktop notifications updated.",
            );
        }
    });
    notification_sound.connect_notify_local(Some("active"), {
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |row: &adw::SwitchRow, _| {
            if suppress_updates.get() {
                return;
            }
            let mut next = current.borrow().clone();
            next.notifications.sound = row.is_active();
            persist_settings_change(
                &dialog,
                &current,
                &on_apply,
                next,
                "Notification sound updated.",
            );
        }
    });
    reset.connect_clicked({
        let dialog = dialog.clone();
        let current = current.clone();
        let on_apply = on_apply.clone();
        let suppress_updates = suppress_updates.clone();
        move |_| {
            let confirmation_parent = dialog.clone();
            let dialog_for_reset = dialog.clone();
            let current_for_reset = current.clone();
            let on_apply_for_reset = on_apply.clone();
            let suppress_updates_for_reset = suppress_updates.clone();
            let shell_entry_for_reset = shell_entry.clone();
            let font_family_for_reset = font_family.clone();
            let font_size_for_reset = font_size.clone();
            let scrollback_lines_for_reset = scrollback_lines.clone();
            let terminal_audible_bell_for_reset = terminal_audible_bell.clone();
            let terminal_theme_for_reset = terminal_theme.clone();
            let window_mode_for_reset = window_mode.clone();
            let sidebar_position_for_reset = sidebar_position.clone();
            let sidebar_visible_for_reset = sidebar_visible.clone();
            let worktree_layout_for_reset = worktree_layout.clone();
            let pr_lookup_for_reset = pr_lookup.clone();
            let notification_command_for_reset = notification_command.clone();
            let desktop_notifications_for_reset = desktop_notifications.clone();
            let notification_sound_for_reset = notification_sound.clone();
            show_destructive_confirmation(
                &confirmation_parent,
                "Reset Settings?",
                "Restore ForkTTY settings to their default values. This changes the saved shell, appearance, workspace, and notification preferences.",
                "Reset Settings",
                move || {
                    let defaults = config::AppConfig::default();
                    suppress_updates_for_reset.set(true);
                    shell_entry_for_reset.set_text(&defaults.general.shell);
                    let _ = font_family_for_reset.set_active_id(Some(DEFAULT_FONT_FAMILY_ID));
                    font_size_for_reset.set_value(i64::from(defaults.appearance.font_size));
                    scrollback_lines_for_reset
                        .set_value(i64::from(defaults.appearance.scrollback_lines));
                    terminal_audible_bell_for_reset
                        .set_active(defaults.appearance.terminal_audible_bell);
                    let _ = terminal_theme_for_reset
                        .set_active_id(Some(&defaults.appearance.terminal_theme));
                    let _ =
                        window_mode_for_reset.set_active_id(Some(&defaults.appearance.window_mode));
                    let _ = sidebar_position_for_reset
                        .set_active_id(Some(&defaults.appearance.sidebar_position));
                    sidebar_visible_for_reset.set_active(defaults.appearance.sidebar_visible);
                    let _ =
                        worktree_layout_for_reset.set_active_id(Some(&defaults.general.worktree_layout));
                    pr_lookup_for_reset.set_active(defaults.general.enable_pr_lookup);
                    notification_command_for_reset.set_text(&defaults.general.notification_command);
                    desktop_notifications_for_reset.set_active(defaults.notifications.desktop);
                    notification_sound_for_reset.set_active(defaults.notifications.sound);
                    suppress_updates_for_reset.set(false);
                    persist_settings_change(
                        &dialog_for_reset,
                        &current_for_reset,
                        &on_apply_for_reset,
                        defaults,
                        "Defaults restored.",
                    );
                },
            );
        }
    });

    dialog.present();
}

pub(super) fn settings_action_row(title: &str, subtitle: &str) -> adw::ActionRow {
    adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .subtitle_lines(0)
        .build()
}

pub(super) fn settings_combo_row(
    title: &str,
    subtitle: &str,
    items: &[(&str, &str)],
    active_id: &str,
) -> (adw::ActionRow, gtk::ComboBoxText) {
    let row = settings_action_row(title, subtitle);
    let combo = combo_with_ids(items, active_id);
    combo.set_valign(gtk::Align::Center);
    combo.set_width_request(180);
    row.add_suffix(&combo);
    row.set_activatable_widget(Some(&combo));
    (row, combo)
}

#[derive(Clone)]
pub(super) struct SettingsNumberControl {
    entry: gtk::Entry,
    decrement: gtk::Button,
    increment: gtk::Button,
    last_value: Rc<Cell<i64>>,
    min: i64,
    max: i64,
    step: i64,
}

impl SettingsNumberControl {
    fn value(&self) -> i64 {
        settings_number_value_from_text(
            &self.entry.text(),
            self.min,
            self.max,
            self.last_value.get(),
        )
    }

    fn set_value(&self, value: i64) {
        let value = value.clamp(self.min, self.max);
        self.last_value.set(value);
        self.entry.set_text(&value.to_string());
    }

    fn stepped_value(&self, delta: i64) -> i64 {
        self.value().saturating_add(delta).clamp(self.min, self.max)
    }
}

pub(super) fn settings_number_value_from_text(
    text: &str,
    min: i64,
    max: i64,
    fallback: i64,
) -> i64 {
    text.trim()
        .parse::<i64>()
        .map(|value| value.clamp(min, max))
        .unwrap_or_else(|_| fallback.clamp(min, max))
}

pub(super) fn settings_number_row(
    title: &str,
    subtitle: &str,
    min: i64,
    max: i64,
    step: i64,
    value: i64,
    width: i32,
) -> (adw::ActionRow, SettingsNumberControl) {
    let row = settings_action_row(title, subtitle);
    let value = value.clamp(min, max);
    let control = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    control.add_css_class("settings-number-control");
    control.set_valign(gtk::Align::Center);
    control.set_halign(gtk::Align::End);

    let entry = gtk::Entry::builder()
        .text(value.to_string())
        .width_request(width)
        .input_purpose(gtk::InputPurpose::Digits)
        .build();
    entry.add_css_class("settings-number-entry");
    gtk::prelude::EntryExt::set_alignment(&entry, 1.0);

    let decrement = gtk::Button::with_label("\u{2212}");
    decrement.add_css_class("settings-number-button");
    decrement.set_tooltip_text(Some("Decrease"));
    set_accessible_button_text(&decrement, "Decrease", None);

    let increment = gtk::Button::with_label("+");
    increment.add_css_class("settings-number-button");
    increment.set_tooltip_text(Some("Increase"));
    set_accessible_button_text(&increment, "Increase", None);

    control.append(&decrement);
    control.append(&entry);
    control.append(&increment);
    row.add_suffix(&control);
    row.set_activatable_widget(Some(&entry));

    (
        row,
        SettingsNumberControl {
            entry,
            decrement,
            increment,
            last_value: Rc::new(Cell::new(value)),
            min,
            max,
            step,
        },
    )
}

pub(super) fn connect_settings_number_control<F>(control: &SettingsNumberControl, apply: F)
where
    F: Fn(i64) + 'static,
{
    let apply: Rc<dyn Fn(i64)> = Rc::new(apply);

    control.decrement.connect_clicked({
        let control = control.clone();
        let apply = apply.clone();
        move |_| {
            let value = control.stepped_value(-control.step);
            control.set_value(value);
            apply(value);
        }
    });

    control.increment.connect_clicked({
        let control = control.clone();
        let apply = apply.clone();
        move |_| {
            let value = control.stepped_value(control.step);
            control.set_value(value);
            apply(value);
        }
    });

    control.entry.connect_activate({
        let control = control.clone();
        let apply = apply.clone();
        move |_| {
            let value = control.value();
            control.set_value(value);
            apply(value);
        }
    });

    let focus = gtk::EventControllerFocus::new();
    focus.connect_leave({
        let control = control.clone();
        move |_| {
            let value = control.value();
            control.set_value(value);
            apply(value);
        }
    });
    control.entry.add_controller(focus);
}

pub(super) fn combo_with_ids(items: &[(&str, &str)], active_id: &str) -> gtk::ComboBoxText {
    let combo = gtk::ComboBoxText::new();
    for (id, label) in items {
        combo.append(Some(id), label);
    }
    if !combo.set_active_id(Some(active_id)) {
        combo.set_active(Some(0));
    }
    combo
}

#[cfg(feature = "browser")]
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

#[cfg(feature = "browser")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BrowserImportDialogParamError {
    NoSources,
    NoData,
}

#[cfg(feature = "browser")]
impl BrowserImportDialogParamError {
    fn message(self) -> &'static str {
        match self {
            Self::NoSources => "Select at least one source profile.",
            Self::NoData => "Select at least one data type to import.",
        }
    }
}

#[cfg(feature = "browser")]
pub(super) struct BrowserImportSourceRow {
    id: String,
    label: String,
    tooltip: Option<String>,
}

#[cfg(feature = "browser")]
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

#[cfg(feature = "browser")]
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

#[cfg(feature = "browser")]
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

#[cfg(feature = "browser")]
pub(super) fn browser_import_count(value: &Value, key: &str) -> usize {
    value.get(key).and_then(Value::as_u64).unwrap_or(0) as usize
}

#[cfg(feature = "browser")]
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

#[cfg(feature = "browser")]
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

pub(super) fn persist_settings_change(
    dialog: &adw::PreferencesWindow,
    current: &Rc<RefCell<config::AppConfig>>,
    on_apply: &SettingsApplyCallback,
    next: config::AppConfig,
    message: &str,
) -> bool {
    if *current.borrow() == next {
        return true;
    }
    match config::save_config(&next) {
        Ok(()) => {
            *current.borrow_mut() = next.clone();
            on_apply(&next);
            dialog.add_toast(adw::Toast::new(message));
            true
        }
        Err(err) => {
            dialog.add_toast(adw::Toast::new(&err.to_string()));
            false
        }
    }
}

pub(super) fn installed_font_family_row_id(name: &str) -> String {
    format!("{INSTALLED_FONT_FAMILY_ID_PREFIX}{name}")
}

pub(super) fn decode_font_family_row_id(id: &str) -> String {
    id.strip_prefix(INSTALLED_FONT_FAMILY_ID_PREFIX)
        .unwrap_or(id)
        .to_string()
}

pub(super) fn font_family_combo(
    parent: &impl IsA<gtk::Widget>,
    active_family: &str,
) -> gtk::ComboBoxText {
    let combo = gtk::ComboBoxText::new();
    let active_family = active_family.trim();
    let all_names = installed_font_families(parent);
    let default_family = default_terminal_font_family(&all_names);
    combo.append(
        Some(DEFAULT_FONT_FAMILY_ID),
        &format!("Default terminal font ({default_family})"),
    );
    let system_monospace =
        resolved_system_monospace_family(parent).unwrap_or_else(|| "monospace".to_string());
    combo.append(
        Some(SYSTEM_MONOSPACE_FONT_FAMILY_ID),
        &format!("System monospace ({system_monospace})"),
    );

    let mut names = installed_monospace_font_families(parent);
    if names.is_empty() {
        names = all_names;
    }
    names.sort_by_key(|name| name.to_ascii_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));

    let mut has_active =
        active_family.is_empty() || active_family.eq_ignore_ascii_case("monospace");
    for name in &names {
        if name == active_family {
            has_active = true;
        }
        combo.append(Some(&installed_font_family_row_id(name)), name);
    }
    if !has_active {
        combo.append(
            Some(&installed_font_family_row_id(active_family)),
            &format!("{active_family} (saved)"),
        );
    }

    let active_id = if active_family.is_empty() {
        DEFAULT_FONT_FAMILY_ID.to_string()
    } else if active_family.eq_ignore_ascii_case("monospace") {
        SYSTEM_MONOSPACE_FONT_FAMILY_ID.to_string()
    } else {
        installed_font_family_row_id(active_family)
    };
    if !combo.set_active_id(Some(&active_id)) {
        combo.set_active(Some(0));
    }
    combo
}
