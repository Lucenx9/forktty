use adw::prelude::*;
use forktty_core::WorkspaceModel;
use forktty_socket::{bootstrap_default_workspace, default_socket_path, SocketAppState};
use forktty_terminal::vte::spawn_vte_terminal;
use forktty_terminal::HeadlessTerminalBackend;
use gtk::glib;
use gtk4 as gtk;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

const APP_ID: &str = "dev.forktty.ForkTTYGtk";

pub fn run() {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run();
}

fn build_ui(app: &adw::Application) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let socket_path = std::env::var("FORKTTY_SOCKET_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_socket_path());

    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(HeadlessTerminalBackend::new());
    let state = SocketAppState::new(model.clone(), backend, shell.clone(), socket_path);
    let _ = bootstrap_default_workspace(&state, cwd);

    let workspace = model
        .lock()
        .ok()
        .and_then(|model| model.list_workspaces().into_iter().next())
        .expect("bootstrap creates a workspace");
    let surface = model
        .lock()
        .ok()
        .and_then(|model| model.surface(&workspace.focused_surface_id).cloned())
        .expect("bootstrap creates a surface");

    let header = adw::HeaderBar::new();
    let split_horizontal = gtk::Button::builder()
        .icon_name("view-split-left-right-symbolic")
        .tooltip_text("Split horizontally")
        .build();
    let split_vertical = gtk::Button::builder()
        .icon_name("view-split-top-bottom-symbolic")
        .tooltip_text("Split vertically")
        .build();
    header.pack_start(&split_horizontal);
    header.pack_start(&split_vertical);

    let sidebar = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .width_request(240)
        .build();
    sidebar.add_css_class("navigation-sidebar");
    let row = gtk::Label::builder()
        .label(&workspace.name)
        .xalign(0.0)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(12)
        .margin_end(12)
        .build();
    sidebar.append(&row);

    let terminal_request = forktty_terminal::SpawnRequest {
        surface_id: surface.id.clone(),
        workspace_id: surface.workspace_id.clone(),
        shell,
        cwd: surface.cwd.clone(),
        socket_path: state.socket_path.clone(),
        extra_env: Vec::new(),
    };
    let terminal = spawn_vte_terminal(&terminal_request).expect("VTE terminal widget");
    terminal.grab_focus();

    let terminal_stack = gtk::Box::new(gtk::Orientation::Vertical, 0);
    terminal_stack.append(&terminal);
    let terminal_stack = Rc::new(RefCell::new(terminal_stack));

    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_start_child(Some(&sidebar));
    paned.set_resize_start_child(false);
    paned.set_shrink_start_child(false);
    paned.set_end_child(Some(&*terminal_stack.borrow()));

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&header);
    content.append(&paned);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("ForkTTY GTK")
        .default_width(1200)
        .default_height(760)
        .content(&content)
        .build();

    let stack_for_horizontal = terminal_stack.clone();
    split_horizontal.connect_clicked(move |_| {
        let current = stack_for_horizontal.borrow();
        let nested = gtk::Paned::new(gtk::Orientation::Horizontal);
        let placeholder = gtk::Label::new(Some("New VTE pane will attach through surface.split"));
        placeholder.add_css_class("dim-label");
        if let Some(child) = current.first_child() {
            child.unparent();
            nested.set_start_child(Some(&child));
            nested.set_end_child(Some(&placeholder));
            current.append(&nested);
        }
    });

    let stack_for_vertical = terminal_stack.clone();
    split_vertical.connect_clicked(move |_| {
        let current = stack_for_vertical.borrow();
        let nested = gtk::Paned::new(gtk::Orientation::Vertical);
        let placeholder = gtk::Label::new(Some("New VTE pane will attach through surface.split"));
        placeholder.add_css_class("dim-label");
        if let Some(child) = current.first_child() {
            child.unparent();
            nested.set_start_child(Some(&child));
            nested.set_end_child(Some(&placeholder));
            current.append(&nested);
        }
    });

    let provider = gtk::CssProvider::new();
    provider.load_from_data(
        "
        window { background: @window_bg_color; }
        .navigation-sidebar { padding: 6px; }
        ",
    );
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    glib::MainContext::default().spawn_local(async move {
        let _ = state;
    });

    window.present();
}
