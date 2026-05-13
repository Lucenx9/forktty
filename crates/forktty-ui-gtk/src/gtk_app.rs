use adw::prelude::*;
use forktty_core::WorkspaceModel;
use forktty_socket::{
    bind_socket_listener, bootstrap_default_workspace, default_socket_path, serve, SocketAppState,
};
use forktty_terminal::vte::{send_text as vte_send_text, spawn_vte_terminal, VteTerminalWidget};
use forktty_terminal::{SpawnRequest, TerminalBackend, TerminalError, TerminalSurfaceState};
use gtk::glib;
use gtk4 as gtk;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

const APP_ID: &str = "dev.forktty.ForkTTYGtk";

#[derive(Debug)]
enum GtkTerminalCommand {
    Spawn(SpawnRequest),
    SendText { surface_id: String, text: String },
    Resize,
}

#[derive(Debug, Clone)]
struct GtkVteBackend {
    sender: mpsc::Sender<GtkTerminalCommand>,
    surfaces: Arc<Mutex<BTreeMap<String, TerminalSurfaceState>>>,
}

impl GtkVteBackend {
    fn new(sender: mpsc::Sender<GtkTerminalCommand>) -> Self {
        Self {
            sender,
            surfaces: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    fn send_command(&self, command: GtkTerminalCommand) -> Result<(), TerminalError> {
        self.sender
            .send(command)
            .map_err(|err| TerminalError::Backend(err.to_string()))
    }
}

impl TerminalBackend for GtkVteBackend {
    fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
        let mut surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        surfaces.insert(
            request.surface_id.clone(),
            TerminalSurfaceState {
                surface_id: request.surface_id.clone(),
                workspace_id: request.workspace_id.clone(),
                cwd: request.cwd.clone(),
                shell: request.shell.clone(),
                cols: 80,
                rows: 24,
            },
        );
        drop(surfaces);
        self.send_command(GtkTerminalCommand::Spawn(request))
    }

    fn send_text(&self, surface_id: &str, text: &str) -> Result<(), TerminalError> {
        if !self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .contains_key(surface_id)
        {
            return Err(TerminalError::NotFound(surface_id.to_string()));
        }
        self.send_command(GtkTerminalCommand::SendText {
            surface_id: surface_id.to_string(),
            text: text.to_string(),
        })
    }

    fn resize(&self, surface_id: &str, cols: u16, rows: u16) -> Result<(), TerminalError> {
        let mut surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        let surface = surfaces
            .get_mut(surface_id)
            .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
        surface.cols = cols;
        surface.rows = rows;
        drop(surfaces);
        self.send_command(GtkTerminalCommand::Resize)
    }

    fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
        let surfaces = self
            .surfaces
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        Ok(surfaces.values().cloned().collect())
    }
}

struct VteController {
    container: gtk::Box,
    widgets: BTreeMap<String, VteTerminalWidget>,
}

impl VteController {
    fn new(container: gtk::Box) -> Self {
        Self {
            container,
            widgets: BTreeMap::new(),
        }
    }

    fn handle(&mut self, command: GtkTerminalCommand) {
        match command {
            GtkTerminalCommand::Spawn(request) => self.spawn(request),
            GtkTerminalCommand::SendText { surface_id, text } => {
                if let Some(widget) = self.widgets.get(&surface_id) {
                    vte_send_text(widget, &text);
                }
            }
            GtkTerminalCommand::Resize => {}
        }
    }

    fn spawn(&mut self, request: SpawnRequest) {
        if self.widgets.contains_key(&request.surface_id) {
            return;
        }
        match spawn_vte_terminal(&request) {
            Ok(widget) => {
                widget.grab_focus();
                self.container.append(&widget);
                self.widgets.insert(request.surface_id, widget);
            }
            Err(err) => eprintln!("Failed to spawn VTE terminal: {err}"),
        }
    }
}

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
    let (terminal_tx, terminal_rx) = mpsc::channel();
    let backend = Arc::new(GtkVteBackend::new(terminal_tx));
    let state = SocketAppState::new(model.clone(), backend, shell.clone(), socket_path);

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
        .label("main")
        .xalign(0.0)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(12)
        .margin_end(12)
        .build();
    sidebar.append(&row);

    let terminal_stack = gtk::Box::new(gtk::Orientation::Vertical, 0);
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

    let controller = Rc::new(RefCell::new(VteController::new(
        terminal_stack.borrow().clone(),
    )));
    let controller_for_timer = controller.clone();
    glib::timeout_add_local(Duration::from_millis(16), move || {
        while let Ok(command) = terminal_rx.try_recv() {
            controller_for_timer.borrow_mut().handle(command);
        }
        glib::ControlFlow::Continue
    });

    if let Err(err) = bootstrap_default_workspace(&state, cwd) {
        eprintln!("Failed to bootstrap default workspace: {err}");
    }

    start_socket_server(state.clone());

    window.present();
}

fn start_socket_server(state: SocketAppState) {
    let listener = match bind_socket_listener(&state.socket_path, true) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!(
                "Failed to bind ForkTTY socket {}: {err}",
                state.socket_path.display()
            );
            return;
        }
    };

    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(err) => {
                eprintln!("Failed to start ForkTTY socket runtime: {err}");
                return;
            }
        };
        if let Err(err) = runtime.block_on(serve(listener, state)) {
            eprintln!("ForkTTY socket server stopped: {err}");
        }
    });
}
