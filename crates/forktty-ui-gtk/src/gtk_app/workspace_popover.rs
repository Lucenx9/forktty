use super::*;

pub(super) fn show_workspace_popover<W: IsA<gtk::Widget>>(
    anchor: &W,
    state: &SocketAppState,
    controller: &Rc<RefCell<VteController>>,
) {
    let popover = gtk::Popover::new();
    popover.add_css_class("ft-workspace-popover");
    popover.set_has_arrow(false);
    popover.set_autohide(true);
    popover.set_parent(anchor);

    let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    container.add_css_class("ft-workspace-popover-list");
    container.set_width_request(280);

    let (active_id, workspaces, ssh_hosts) = {
        let Ok(model) = state.model.lock() else {
            return;
        };
        let workspaces = model.list_workspaces();
        let ssh_hosts: std::collections::BTreeMap<String, String> = workspaces
            .iter()
            .filter_map(|ws| {
                model.list_surfaces(Some(&ws.id)).into_iter().find_map(|s| {
                    if let forktty_core::SurfaceKind::Ssh { host } = s.kind {
                        Some((ws.id.clone(), host))
                    } else {
                        None
                    }
                })
            })
            .collect();
        (model.active_workspace_id(), workspaces, ssh_hosts)
    };

    if workspaces.is_empty() {
        let empty = gtk::Label::builder().label("No workspaces").build();
        empty.add_css_class("ft-workspace-popover-empty");
        container.append(&empty);
    } else {
        for ws in workspaces {
            let is_active = active_id.as_deref() == Some(ws.id.as_str());
            let row = gtk::Button::builder().has_frame(false).build();
            row.add_css_class("flat");
            row.add_css_class("ft-workspace-popover-row");
            if is_active {
                row.add_css_class("active");
            }

            let inner = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            inner.add_css_class("ft-workspace-popover-row-inner");

            let check = gtk::Image::from_icon_name("forktty-check-symbolic");
            check.add_css_class("ft-workspace-popover-check");
            check.set_visible(is_active);
            inner.append(&check);

            let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
            body.set_hexpand(true);
            let name = gtk::Label::builder()
                .label(&ws.name)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            name.add_css_class("ft-workspace-popover-name");
            let mut meta_parts: Vec<String> = Vec::new();
            if let Some(host) = ssh_hosts.get(&ws.id) {
                meta_parts.push(format!("ssh:{host}"));
            }
            let branch = ws.git_branch.trim();
            if !branch.is_empty() {
                meta_parts.push(branch.to_string());
            }
            if let Some(wt) = ws.worktree_name.as_deref() {
                let wt = wt.trim();
                if !wt.is_empty() {
                    meta_parts.push(format!("wt:{wt}"));
                }
            }
            body.append(&name);
            meta_parts.push(compact_path(&ws.working_dir));
            let meta = meta_parts.join(" · ");
            if meta != "~" {
                let path = gtk::Label::builder()
                    .label(meta)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .build();
                path.add_css_class("ft-workspace-popover-path");
                body.append(&path);
            }
            inner.append(&body);
            row.set_child(Some(&inner));

            let popover_for_row = popover.clone();
            let state_for_row = state.clone();
            let controller_for_row = controller.clone();
            let ws_id = ws.id.clone();
            row.connect_clicked(move |_| {
                popover_for_row.popdown();
                select_sidebar_workspace(&state_for_row, &ws_id, &controller_for_row);
            });
            container.append(&row);
        }
    }

    let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
    separator.add_css_class("ft-workspace-popover-separator");
    container.append(&separator);

    let new_btn = gtk::Button::builder().has_frame(false).build();
    new_btn.add_css_class("flat");
    new_btn.add_css_class("ft-workspace-popover-row");
    new_btn.add_css_class("ft-workspace-popover-action");
    let new_inner = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    new_inner.add_css_class("ft-workspace-popover-row-inner");
    let new_icon = gtk::Image::from_icon_name("forktty-add-symbolic");
    new_icon.add_css_class("ft-workspace-popover-action-icon");
    new_inner.append(&new_icon);
    let new_label = gtk::Label::builder()
        .label("New Workspace")
        .xalign(0.0)
        .hexpand(true)
        .build();
    new_inner.append(&new_label);
    new_btn.set_child(Some(&new_inner));
    let popover_for_new = popover.clone();
    let state_for_new = state.clone();
    new_btn.connect_clicked(move |_| {
        popover_for_new.popdown();
        create_plain_workspace(&state_for_new);
    });
    container.append(&new_btn);

    popover.set_child(Some(&container));

    let popover_for_cleanup = popover.clone();
    popover.connect_closed(move |_| {
        popover_for_cleanup.unparent();
    });

    popover.popup();
}
