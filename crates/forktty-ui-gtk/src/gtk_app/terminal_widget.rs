use super::*;
use forktty_terminal::ghostty::core::{
    TerminalMouseAction, TerminalMouseButton, TerminalMouseInput, TerminalMousePosition,
    TerminalMouseSize,
};
use forktty_terminal::ghostty::events::GhosttyEvent;

/// Blink half-period for the focused cursor: visible for one interval, hidden
/// for the next. Matches the conventional terminal cadence.
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(530);

#[derive(Debug, Clone)]
pub(super) struct GhosttyTerminalWidget {
    drawing_area: gtk::DrawingArea,
    runtime: Rc<RefCell<TerminalRuntime>>,
    selection: Rc<RefCell<TerminalSelection>>,
    // Blink phase for the focused cursor; `true` means the cursor is drawn.
    cursor_blink_visible: Rc<Cell<bool>>,
    // Index of the active scrollback search match; matches themselves are
    // recomputed on every step so new output never leaves stale positions.
    search_index: Rc<Cell<Option<usize>>>,
    // Scrollback dump + matches reused across steps while content and query
    // are unchanged; see `SearchCache`.
    search_cache: Rc<RefCell<Option<SearchCache>>>,
    // Notifies the search bar when new output drops the match highlight, so
    // its count label resets instead of showing a stale "current/total".
    search_invalidated: Rc<SearchInvalidatedSlot>,
}

#[derive(Default)]
pub(super) struct SearchInvalidatedSlot {
    callback: RefCell<Option<Box<dyn Fn()>>>,
}

impl std::fmt::Debug for SearchInvalidatedSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SearchInvalidatedSlot")
    }
}

impl SearchInvalidatedSlot {
    fn invoke(&self) {
        // The borrow is held across the call; safe because the callback only
        // touches GTK label/entry state and never re-enters this slot.
        if let Some(callback) = self.callback.borrow().as_ref() {
            callback();
        }
    }
}

pub(super) fn spawn_terminal_with_callback<F>(
    request: &SpawnRequest,
    on_spawn_result: F,
) -> Result<GhosttyTerminalWidget, TerminalError>
where
    F: FnOnce(Result<TerminalSpawnPid, TerminalError>) + 'static,
{
    spawn_terminal_with_callback_impl(request, on_spawn_result)
}

fn spawn_terminal_with_callback_impl<F>(
    request: &SpawnRequest,
    on_spawn_result: F,
) -> Result<GhosttyTerminalWidget, TerminalError>
where
    F: FnOnce(Result<TerminalSpawnPid, TerminalError>) + 'static,
{
    let config = config::load_config().unwrap_or_default();
    let runtime = TerminalRuntime::spawn_with_scrollback_lines(
        request,
        forktty_terminal::ghostty::pty::PtySize { cols: 80, rows: 24 },
        config.appearance.scrollback_lines as usize,
    )?;
    let pid = runtime.child_pid();
    let widget = GhosttyTerminalWidget::new_with_config(runtime, &config);
    on_spawn_result(Ok(pid));
    Ok(widget)
}

impl GhosttyTerminalWidget {
    fn new_with_config(runtime: TerminalRuntime, config: &config::AppConfig) -> Self {
        let drawing_area = gtk::DrawingArea::new();
        drawing_area.set_hexpand(true);
        drawing_area.set_vexpand(true);
        drawing_area.set_focusable(true);
        drawing_area.add_css_class("ghostty-terminal");
        let runtime = Rc::new(RefCell::new(runtime));
        let selection = Rc::new(RefCell::new(TerminalSelection::default()));
        let cursor_blink_visible = Rc::new(Cell::new(true));
        let font = terminal_font_description(&drawing_area, config);
        let renderer = TerminalRenderer::from_config_with_font(config, font);
        let im_context = gtk::IMMulticontext::new();
        im_context.set_client_widget(Some(&drawing_area));
        {
            let runtime = runtime.clone();
            let renderer = renderer.clone();
            let selection = selection.clone();
            let cursor_blink_visible = cursor_blink_visible.clone();
            drawing_area.set_draw_func(move |area, cr, width, height| {
                let frame = runtime.borrow_mut().render_frame();
                match frame {
                    Ok(frame) => {
                        let range = selection.borrow().normalized_range();
                        renderer.draw_frame(
                            cr,
                            width,
                            height,
                            &frame,
                            range,
                            RendererCursorState {
                                focused: area.has_focus(),
                                blink_visible: cursor_blink_visible.get(),
                            },
                        );
                    }
                    Err(err) => eprintln!("Failed to render terminal frame: {err}"),
                }
            });
        }
        {
            let runtime = runtime.clone();
            let drawing_area_for_key = drawing_area.downgrade();
            let key_controller = gtk::EventControllerKey::new();
            key_controller.set_im_context(Some(&im_context));
            key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
            im_context.connect_commit({
                let runtime = runtime.clone();
                let selection = selection.clone();
                let drawing_area = drawing_area.downgrade();
                let cursor_blink_visible = cursor_blink_visible.clone();
                move |_context, text| {
                    let Some(drawing_area) = drawing_area.upgrade() else {
                        return;
                    };
                    let Some(input) = terminal_text_input(text) else {
                        return;
                    };
                    // Typing snaps the blink phase to visible so the cursor
                    // never disappears mid-keystroke.
                    cursor_blink_visible.set(true);
                    write_terminal_input(&runtime, &selection, &drawing_area, input);
                }
            });
            let cursor_blink_visible_for_key = cursor_blink_visible.clone();
            let selection_for_key = selection.clone();
            key_controller.connect_key_pressed(move |_, key, _keycode, modifiers| {
                // With an IM context installed, EventControllerKey runs
                // IMContext::filter_keypress before ::key-pressed; consumed
                // dead-key/compose events emit ::im-update and never reach
                // this handler, while completed text arrives via ::commit.
                if let Some(navigation) = scrollback_navigation_for_key(key, modifiers) {
                    let Some(drawing_area) = drawing_area_for_key.upgrade() else {
                        return glib::Propagation::Proceed;
                    };
                    match handle_scrollback_navigation(&runtime, &selection_for_key, navigation) {
                        Ok(true) => {
                            drawing_area.queue_draw();
                            return glib::Propagation::Stop;
                        }
                        // Alternate screen: the key belongs to the application below.
                        Ok(false) => {}
                        Err(err) => {
                            eprintln!("Failed to navigate terminal scrollback: {err}");
                            // The key was claimed by scrollback navigation; never leak it into
                            // the shell on a backend error.
                            return glib::Propagation::Stop;
                        }
                    }
                }
                let Some(input) = translate_gtk_key(key, modifiers, None) else {
                    return glib::Propagation::Proceed;
                };
                let Some(drawing_area) = drawing_area_for_key.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                cursor_blink_visible_for_key.set(true);
                write_terminal_input(&runtime, &selection_for_key, &drawing_area, input);
                glib::Propagation::Stop
            });
            drawing_area.add_controller(key_controller);
        }
        {
            let runtime_for_enter = runtime.clone();
            let runtime_for_leave = runtime.clone();
            let drawing_area_for_enter = drawing_area.downgrade();
            let drawing_area_for_leave = drawing_area.downgrade();
            let im_context_for_enter = im_context.clone();
            let im_context_for_leave = im_context.clone();
            let cursor_blink_visible_for_enter = cursor_blink_visible.clone();
            let focus_controller = gtk::EventControllerFocus::new();
            focus_controller.connect_enter(move |_| {
                let Some(drawing_area) = drawing_area_for_enter.upgrade() else {
                    return;
                };
                im_context_for_enter.focus_in();
                cursor_blink_visible_for_enter.set(true);
                if let Err(err) = runtime_for_enter.borrow_mut().write_focus(true) {
                    eprintln!("Failed to write terminal focus input: {err}");
                }
                drawing_area.queue_draw();
            });
            focus_controller.connect_leave(move |_| {
                let Some(drawing_area) = drawing_area_for_leave.upgrade() else {
                    return;
                };
                im_context_for_leave.focus_out();
                if let Err(err) = runtime_for_leave.borrow_mut().write_focus(false) {
                    eprintln!("Failed to write terminal focus input: {err}");
                }
                drawing_area.queue_draw();
            });
            drawing_area.add_controller(focus_controller);
        }
        {
            let any_button_pressed = Rc::new(Cell::new(false));
            // Set on a word/line click selection whose press was not
            // forwarded to the application; the matching release must then
            // not be forwarded either.
            let suppress_release = Rc::new(Cell::new(false));
            let autoscroll = Rc::new(SelectionAutoscroll::default());
            let click = gtk::GestureClick::new();
            click.set_button(0);
            click.set_propagation_phase(gtk::PropagationPhase::Capture);
            {
                let runtime = runtime.clone();
                let renderer = renderer.clone();
                let drawing_area = drawing_area.downgrade();
                let any_button_pressed = any_button_pressed.clone();
                let suppress_release = suppress_release.clone();
                let autoscroll = autoscroll.clone();
                let selection = selection.clone();
                click.connect_pressed(move |gesture, n_press, x, y| {
                    let Some(drawing_area) = drawing_area.upgrade() else {
                        return;
                    };
                    let Some(button) = terminal_mouse_button(gesture.current_button()) else {
                        return;
                    };
                    any_button_pressed.set(true);
                    let modifiers = gesture.current_event_state();
                    let is_left = matches!(button, TerminalMouseButton::Left);
                    // Shift bypasses application mouse tracking so text can be
                    // selected even inside mouse-aware apps (vim, htop, ...).
                    let shift_select =
                        is_left && modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK);
                    let forwarded = if shift_select {
                        false
                    } else {
                        let input = terminal_mouse_input_for_area(
                            &drawing_area,
                            &renderer,
                            TerminalMouseEventInput {
                                action: TerminalMouseAction::Press,
                                button: Some(button),
                                modifiers,
                                x,
                                y,
                                any_button_pressed: true,
                            },
                        );
                        write_terminal_mouse(&runtime, &drawing_area, input)
                    };
                    if is_left {
                        let mut selection = selection.borrow_mut();
                        if forwarded {
                            selection.clear();
                        } else {
                            // A fresh gesture must not inherit a previous
                            // drag's pending autoscroll.
                            autoscroll.lines.set(0);
                            let point =
                                selection_cell_for_position(&drawing_area, &renderer, x, y);
                            if n_press == 1 {
                                selection.begin_drag(point);
                            } else {
                                // Double click selects the word, triple click
                                // the visual row. Both finish immediately, so
                                // the release needs explicit suppression.
                                suppress_release.set(true);
                                let frame = runtime.borrow_mut().render_frame();
                                match frame {
                                    Ok(frame) => {
                                        let range = if n_press == 2 {
                                            word_selection_in_frame(&frame, point)
                                        } else {
                                            line_selection_in_frame(&frame, point.row)
                                        };
                                        match range {
                                            Some((start, end)) => commit_selection_range(
                                                &mut selection,
                                                &frame,
                                                start,
                                                end,
                                            ),
                                            None => selection.clear(),
                                        }
                                    }
                                    Err(err) => {
                                        eprintln!(
                                            "Failed to render terminal frame for click selection: {err}"
                                        );
                                        selection.clear();
                                    }
                                }
                            }
                        }
                        drawing_area.queue_draw();
                    }
                });
            }
            {
                let runtime = runtime.clone();
                let renderer = renderer.clone();
                let drawing_area = drawing_area.downgrade();
                let any_button_pressed = any_button_pressed.clone();
                let suppress_release = suppress_release.clone();
                let selection = selection.clone();
                click.connect_released(move |gesture, _n_press, x, y| {
                    let Some(drawing_area) = drawing_area.upgrade() else {
                        return;
                    };
                    let Some(button) = terminal_mouse_button(gesture.current_button()) else {
                        return;
                    };
                    any_button_pressed.set(false);
                    if matches!(button, TerminalMouseButton::Left) {
                        if selection.borrow().is_selecting() {
                            // The press was not forwarded to the application,
                            // so the release must not be either.
                            finish_selection_drag(
                                &runtime,
                                &selection,
                                &drawing_area,
                                &renderer,
                                x,
                                y,
                            );
                            return;
                        }
                        // Same rule for a word/line click selection, which
                        // finished during the press.
                        if suppress_release.replace(false) {
                            return;
                        }
                    }
                    let input = terminal_mouse_input_for_area(
                        &drawing_area,
                        &renderer,
                        TerminalMouseEventInput {
                            action: TerminalMouseAction::Release,
                            button: Some(button),
                            modifiers: gesture.current_event_state(),
                            x,
                            y,
                            any_button_pressed: false,
                        },
                    );
                    write_terminal_mouse(&runtime, &drawing_area, input);
                });
            }
            drawing_area.add_controller(click);

            let motion = gtk::EventControllerMotion::new();
            motion.set_propagation_phase(gtk::PropagationPhase::Capture);
            {
                let runtime = runtime.clone();
                let renderer = renderer.clone();
                let drawing_area = drawing_area.downgrade();
                let any_button_pressed = any_button_pressed.clone();
                let autoscroll = autoscroll.clone();
                let selection = selection.clone();
                motion.connect_motion(move |controller, x, y| {
                    let Some(drawing_area) = drawing_area.upgrade() else {
                        return;
                    };
                    if selection.borrow().is_selecting() {
                        selection
                            .borrow_mut()
                            .extend_drag(selection_cell_for_position(
                                &drawing_area,
                                &renderer,
                                x,
                                y,
                            ));
                        // Steer drag-autoscroll: past the top/bottom edge a
                        // timer scrolls and keeps extending the selection
                        // until the pointer comes back inside.
                        autoscroll.pointer.set((x, y));
                        let lines = autoscroll_lines_per_tick(
                            y,
                            f64::from(drawing_area.allocated_height()),
                        );
                        autoscroll.lines.set(lines);
                        if lines != 0 && !autoscroll.active.get() {
                            autoscroll.active.set(true);
                            spawn_selection_autoscroll_timer(
                                &drawing_area,
                                &runtime,
                                &selection,
                                &renderer,
                                &autoscroll,
                            );
                        }
                        drawing_area.queue_draw();
                        return;
                    }
                    let input = terminal_mouse_input_for_area(
                        &drawing_area,
                        &renderer,
                        TerminalMouseEventInput {
                            action: TerminalMouseAction::Motion,
                            button: None,
                            modifiers: controller.current_event_state(),
                            x,
                            y,
                            any_button_pressed: any_button_pressed.get(),
                        },
                    );
                    write_terminal_mouse(&runtime, &drawing_area, input);
                });
            }
            drawing_area.add_controller(motion);

            let scroll = gtk::EventControllerScroll::new(
                gtk::EventControllerScrollFlags::VERTICAL
                    | gtk::EventControllerScrollFlags::DISCRETE,
            );
            scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
            {
                let runtime = runtime.clone();
                let renderer = renderer.clone();
                let drawing_area = drawing_area.downgrade();
                let selection = selection.clone();
                scroll.connect_scroll(move |controller, _dx, dy| {
                    let Some(drawing_area) = drawing_area.upgrade() else {
                        return glib::Propagation::Proceed;
                    };
                    let Some(button) = terminal_scroll_button(dy) else {
                        return glib::Propagation::Proceed;
                    };
                    let (x, y) = controller
                        .current_event()
                        .and_then(|event| event.position())
                        .unwrap_or((0.0, 0.0));
                    let input = terminal_mouse_input_for_area(
                        &drawing_area,
                        &renderer,
                        TerminalMouseEventInput {
                            action: TerminalMouseAction::Press,
                            button: Some(button),
                            modifiers: controller.current_event_state(),
                            x,
                            y,
                            any_button_pressed: false,
                        },
                    );
                    match route_terminal_scroll(&runtime, input, dy) {
                        Ok(ScrollRouting::Forwarded) => {
                            drawing_area.queue_draw();
                            glib::Propagation::Stop
                        }
                        Ok(ScrollRouting::ViewportScrolled(scrolled)) => {
                            if scrolled {
                                // Selection coordinates are viewport-relative;
                                // scrolling would leave the highlight on the
                                // wrong text.
                                selection.borrow_mut().clear();
                                drawing_area.queue_draw();
                            }
                            glib::Propagation::Stop
                        }
                        Ok(ScrollRouting::NotHandled) => glib::Propagation::Proceed,
                        Err(err) => {
                            eprintln!("Failed to route terminal scroll input: {err}");
                            glib::Propagation::Proceed
                        }
                    }
                });
            }
            drawing_area.add_controller(scroll);
        }
        {
            let runtime = runtime.clone();
            let renderer = renderer.clone();
            drawing_area.connect_resize(move |area, width, height| {
                let (cell_width, cell_height) = renderer.cell_pixel_size_for_widget(area);
                if let Err(err) =
                    runtime
                        .borrow_mut()
                        .resize_pixels(width, height, cell_width, cell_height)
                {
                    eprintln!("Failed to resize terminal runtime: {err}");
                }
                area.queue_draw();
            });
        }
        let widget = Self {
            drawing_area,
            runtime,
            selection,
            cursor_blink_visible,
            search_index: Rc::new(Cell::new(None)),
            search_cache: Rc::new(RefCell::new(None)),
            search_invalidated: Rc::new(SearchInvalidatedSlot::default()),
        };
        widget.attach_cursor_blink_timer();
        widget
    }

    // Flips the blink phase while the pane is focused; an unfocused pane keeps
    // its (hollow) cursor steadily visible. The weak handle lets the timer die
    // with the widget instead of keeping a closed pane alive.
    fn attach_cursor_blink_timer(&self) {
        let blink_widget_weak = self.downgrade_widget();
        glib::timeout_add_local(CURSOR_BLINK_INTERVAL, move || {
            let Some(blink_widget) = blink_widget_weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if blink_widget.drawing_area.has_focus() {
                blink_widget
                    .cursor_blink_visible
                    .set(!blink_widget.cursor_blink_visible.get());
                blink_widget.drawing_area.queue_draw();
            } else if !blink_widget.cursor_blink_visible.get() {
                blink_widget.cursor_blink_visible.set(true);
                blink_widget.drawing_area.queue_draw();
            }
            glib::ControlFlow::Continue
        });
    }

    pub(super) fn downgrade(&self) -> glib::WeakRef<gtk::DrawingArea> {
        self.drawing_area.downgrade()
    }

    pub(super) fn downgrade_widget(&self) -> WeakGhosttyTerminalWidget {
        WeakGhosttyTerminalWidget {
            drawing_area: self.drawing_area.downgrade(),
            runtime: Rc::downgrade(&self.runtime),
            selection: Rc::downgrade(&self.selection),
            cursor_blink_visible: Rc::downgrade(&self.cursor_blink_visible),
            search_index: Rc::downgrade(&self.search_index),
            search_cache: Rc::downgrade(&self.search_cache),
            search_invalidated: Rc::downgrade(&self.search_invalidated),
        }
    }

    /// Registers the search bar's reaction to a pump-side highlight drop.
    /// The callback must not capture the widget (that would leak an Rc
    /// cycle: widget → slot → widget).
    pub(super) fn on_search_invalidated(&self, callback: impl Fn() + 'static) {
        *self.search_invalidated.callback.borrow_mut() = Some(Box::new(callback));
    }

    pub(super) fn attach_navigation_key_fallback<W>(&self, target: &W)
    where
        W: IsA<gtk::Widget>,
    {
        let widget = self.downgrade_widget();
        let key_controller = gtk::EventControllerKey::new();
        key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        key_controller.connect_key_pressed(move |_, key, _keycode, modifiers| {
            let Some(input) = translate_gtk_navigation_key(key, modifiers) else {
                return glib::Propagation::Proceed;
            };
            let Some(widget) = widget.upgrade() else {
                return glib::Propagation::Proceed;
            };
            forward_terminal_navigation_input(&widget, input);
            glib::Propagation::Stop
        });
        target.add_controller(key_controller);
    }

    fn with_runtime(&self, f: impl FnOnce(&mut TerminalRuntime) -> Result<(), TerminalError>) {
        if let Err(err) = f(&mut self.runtime.borrow_mut()) {
            eprintln!("Terminal runtime operation failed: {err}");
        }
        self.drawing_area.queue_draw();
    }

    /// Steps the scrollback search to the next (or previous) match of
    /// `query`, scrolls it into view, and highlights it via the selection so
    /// the renderer paints it and Ctrl+Shift+C copies it. Matches are
    /// recomputed whenever the terminal content or the query changed since
    /// the last step (so new output never leaves stale positions) and reused
    /// otherwise — a full-scrollback dump per step is too slow at 100k lines.
    pub(super) fn search_step(&self, query: &str, forward: bool) -> SearchStatus {
        let generation = self.runtime.borrow().content_generation();
        let mut cache_slot = self.search_cache.borrow_mut();
        match cache_slot.as_mut() {
            Some(cache) if cache.generation == generation && cache.query == query => {}
            Some(cache) if cache.generation == generation => {
                cache.matches = find_matches(&cache.text, query);
                cache.query = query.to_string();
            }
            _ => {
                let text = self.runtime.borrow().full_text();
                let matches = find_matches(&text, query);
                *cache_slot = Some(SearchCache {
                    generation,
                    text,
                    query: query.to_string(),
                    matches,
                });
            }
        }
        let matches = &cache_slot
            .as_ref()
            .expect("search cache was just populated")
            .matches;
        let Some(index) = step_match_index(self.search_index.get(), matches.len(), forward) else {
            drop(cache_slot);
            self.search_reset();
            return SearchStatus::none();
        };
        let total = matches.len();
        let search_match = matches[index];
        drop(cache_slot);
        self.search_index.set(Some(index));
        if let Err(err) = self.show_search_match(query, search_match) {
            eprintln!("Failed to show terminal search match: {err}");
        }
        SearchStatus {
            current: index + 1,
            total,
        }
    }

    /// Forgets the active match and drops its highlight. The cached scrollback
    /// dump survives: this runs on every query change, where the cache is what
    /// keeps incremental typing cheap.
    pub(super) fn search_reset(&self) {
        self.search_index.set(None);
        self.selection.borrow_mut().clear();
        self.drawing_area.queue_draw();
    }

    /// `search_reset` plus dropping the cached scrollback dump; for when the
    /// search bar closes and the memory should be released.
    pub(super) fn search_close(&self) {
        self.search_reset();
        *self.search_cache.borrow_mut() = None;
    }

    fn show_search_match(
        &self,
        query: &str,
        search_match: SearchMatch,
    ) -> Result<(), TerminalError> {
        let mut runtime = self.runtime.borrow_mut();
        let viewport = runtime.viewport_position()?;
        let top = viewport_top_for_match(search_match.line, viewport);
        if top != viewport.top {
            runtime.scroll_viewport_lines(top as isize - viewport.top as isize)?;
        }
        // Re-read the position in case the core clamped the scroll.
        let top = runtime.viewport_position()?.top;
        let frame = runtime.render_frame()?;
        drop(runtime);
        let row = search_match.line.saturating_sub(top);
        if let Some(frame_row) = frame.rows.get(row) {
            let cell_texts: Vec<&str> = frame_row
                .cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect();
            if let Some((from, to)) =
                match_cells_in_row(&cell_texts, query, search_match.occurrence)
            {
                let mut selection = self.selection.borrow_mut();
                selection.begin_drag(SelectionPoint { row, col: from });
                selection.extend_drag(SelectionPoint { row, col: to });
                selection.end_drag();
                selection.select_text(cell_texts[from..=to].concat());
            }
        }
        self.drawing_area.queue_draw();
        Ok(())
    }

    pub(super) fn pump_pty_events(&self) -> Result<Vec<GhosttyEvent>, TerminalError> {
        let events = self.runtime.borrow_mut().pump_pty()?;
        if !events.is_empty() {
            // New output shifts the viewport content under a finished
            // selection; drop it rather than highlight the wrong text.
            let mut selection_cleared = false;
            {
                let mut selection = self.selection.borrow_mut();
                if !selection.is_selecting()
                    && selection.normalized_range().is_some()
                    && events
                        .iter()
                        .any(|event| matches!(event, GhosttyEvent::VisibleContentChanged))
                {
                    selection.clear();
                    selection_cleared = true;
                }
            }
            // The dropped selection may have been the search-match highlight;
            // reset the match state and tell the search bar so its count
            // label follows. Outside the selection borrow: the callback runs
            // foreign (GTK) code.
            if selection_cleared && self.search_index.get().is_some() {
                self.search_index.set(None);
                self.search_invalidated.invoke();
            }
            self.drawing_area.queue_draw();
        }
        Ok(events)
    }
}

// A weak handle over every field of the widget so background work (e.g. the PTY
// pump timer) can keep running without keeping the terminal runtime and PTY alive
// after the pane is closed. `upgrade()` returns `None` once the widget is dropped.
#[derive(Clone)]
pub(super) struct WeakGhosttyTerminalWidget {
    drawing_area: glib::WeakRef<gtk::DrawingArea>,
    runtime: std::rc::Weak<RefCell<TerminalRuntime>>,
    selection: std::rc::Weak<RefCell<TerminalSelection>>,
    cursor_blink_visible: std::rc::Weak<Cell<bool>>,
    search_index: std::rc::Weak<Cell<Option<usize>>>,
    search_cache: std::rc::Weak<RefCell<Option<SearchCache>>>,
    search_invalidated: std::rc::Weak<SearchInvalidatedSlot>,
}

impl WeakGhosttyTerminalWidget {
    pub(super) fn upgrade(&self) -> Option<GhosttyTerminalWidget> {
        Some(GhosttyTerminalWidget {
            drawing_area: self.drawing_area.upgrade()?,
            runtime: self.runtime.upgrade()?,
            selection: self.selection.upgrade()?,
            cursor_blink_visible: self.cursor_blink_visible.upgrade()?,
            search_index: self.search_index.upgrade()?,
            search_cache: self.search_cache.upgrade()?,
            search_invalidated: self.search_invalidated.upgrade()?,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct TerminalMouseEventInput {
    action: TerminalMouseAction,
    button: Option<TerminalMouseButton>,
    modifiers: gtk::gdk::ModifierType,
    x: f64,
    y: f64,
    any_button_pressed: bool,
}

#[derive(Debug, Clone, Copy)]
struct TerminalMouseWidgetMetrics {
    screen_width: i32,
    screen_height: i32,
    cell_width: i32,
    cell_height: i32,
}

fn terminal_mouse_input_for_area(
    area: &gtk::DrawingArea,
    renderer: &TerminalRenderer,
    event: TerminalMouseEventInput,
) -> TerminalMouseInput {
    let (cell_width, cell_height) = renderer.cell_pixel_size_for_widget(area);
    terminal_mouse_input(
        event,
        TerminalMouseWidgetMetrics {
            screen_width: area.allocated_width(),
            screen_height: area.allocated_height(),
            cell_width,
            cell_height,
        },
    )
}

fn terminal_mouse_input(
    event: TerminalMouseEventInput,
    metrics: TerminalMouseWidgetMetrics,
) -> TerminalMouseInput {
    TerminalMouseInput {
        action: event.action,
        button: event.button,
        modifiers: terminal_key_modifiers(event.modifiers),
        position: TerminalMousePosition {
            x: event.x.max(0.0) as f32,
            y: event.y.max(0.0) as f32,
        },
        size: TerminalMouseSize {
            screen_width: positive_u32(metrics.screen_width),
            screen_height: positive_u32(metrics.screen_height),
            cell_width: positive_u32(metrics.cell_width),
            cell_height: positive_u32(metrics.cell_height),
        },
        any_button_pressed: event.any_button_pressed,
    }
}

fn positive_u32(value: i32) -> u32 {
    value.max(1) as u32
}

fn write_terminal_mouse(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    drawing_area: &gtk::DrawingArea,
    input: TerminalMouseInput,
) -> bool {
    match runtime.borrow_mut().write_mouse(input) {
        Ok(wrote) => {
            if wrote {
                drawing_area.queue_draw();
            }
            wrote
        }
        Err(err) => {
            eprintln!("Failed to write terminal mouse input: {err}");
            false
        }
    }
}

fn selection_cell_for_position(
    area: &gtk::DrawingArea,
    renderer: &TerminalRenderer,
    x: f64,
    y: f64,
) -> SelectionPoint {
    let (cell_width, cell_height) = renderer.cell_pixel_size_for_widget(area);
    SelectionPoint {
        row: (y.max(0.0) / f64::from(cell_height.max(1))) as usize,
        col: (x.max(0.0) / f64::from(cell_width.max(1))) as usize,
    }
}

fn finish_selection_drag(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    selection: &Rc<RefCell<TerminalSelection>>,
    drawing_area: &gtk::DrawingArea,
    renderer: &TerminalRenderer,
    x: f64,
    y: f64,
) {
    let mut selection = selection.borrow_mut();
    selection.extend_drag(selection_cell_for_position(drawing_area, renderer, x, y));
    selection.end_drag();
    match selection.normalized_range() {
        Some((start, end)) if start != end => match runtime.borrow_mut().render_frame() {
            Ok(frame) => commit_selection_range(&mut selection, &frame, start, end),
            Err(err) => {
                eprintln!("Failed to extract terminal selection: {err}");
                selection.clear();
            }
        },
        // A plain click (no drag) leaves no selection behind.
        _ => selection.clear(),
    }
    drawing_area.queue_draw();
}

/// The viewport range a double click at `point` selects: the word (or
/// whitespace/punctuation run) around the clicked cell, ghostty-style.
fn word_selection_in_frame(
    frame: &forktty_terminal::ghostty::core::TerminalFrame,
    point: SelectionPoint,
) -> Option<(SelectionPoint, SelectionPoint)> {
    use forktty_terminal::ghostty::core::TerminalCellWidth;
    let row = frame.rows.get(point.row)?;
    let kinds: Vec<WordCellKind> = row
        .cells
        .iter()
        .map(|cell| word_cell_kind(&cell.text, cell.width == TerminalCellWidth::SpacerTail))
        .collect();
    let (from, to) = word_cols_in_row(&kinds, point.col)?;
    Some((
        SelectionPoint {
            row: point.row,
            col: from,
        },
        SelectionPoint {
            row: point.row,
            col: to,
        },
    ))
}

/// The viewport range a triple click on `row` selects: the whole visual row.
/// Logical (soft-wrapped) lines are out of scope.
fn line_selection_in_frame(
    frame: &forktty_terminal::ghostty::core::TerminalFrame,
    row: usize,
) -> Option<(SelectionPoint, SelectionPoint)> {
    let cells = frame.rows.get(row)?.cells.len();
    Some((
        SelectionPoint { row, col: 0 },
        SelectionPoint {
            row,
            col: cells.checked_sub(1)?,
        },
    ))
}

/// Installs `start..=end` as the finished selection and stores its text,
/// publishing it to the PRIMARY clipboard like a finished drag. Blank text
/// clears the selection instead.
fn commit_selection_range(
    selection: &mut TerminalSelection,
    frame: &forktty_terminal::ghostty::core::TerminalFrame,
    start: SelectionPoint,
    end: SelectionPoint,
) {
    selection.begin_drag(start);
    selection.extend_drag(end);
    selection.end_drag();
    let text = selection_text_from_frame(frame, start, end);
    if text.trim().is_empty() {
        selection.clear();
        return;
    }
    selection.select_text(text.clone());
    if let Some(display) = gtk::gdk::Display::default() {
        display.primary_clipboard().set_text(&text);
    }
}

/// Drag-autoscroll cadence; the per-tick speed is `autoscroll_lines_per_tick`.
const SELECTION_AUTOSCROLL_INTERVAL: Duration = Duration::from_millis(75);

/// Shared between the motion handler (which steers) and the autoscroll timer
/// (which scrolls): the pending per-tick line delta, the last pointer
/// position, and whether a timer is currently alive.
#[derive(Debug, Default)]
struct SelectionAutoscroll {
    lines: Cell<isize>,
    pointer: Cell<(f64, f64)>,
    active: Cell<bool>,
}

/// Scrolls the viewport while a selection drag sits past the top or bottom
/// edge. Like the pump and blink timers, the closure only holds weak
/// references, so it dies with the pane; it also stops on release or once the
/// pointer comes back inside.
fn spawn_selection_autoscroll_timer(
    drawing_area: &gtk::DrawingArea,
    runtime: &Rc<RefCell<TerminalRuntime>>,
    selection: &Rc<RefCell<TerminalSelection>>,
    renderer: &TerminalRenderer,
    autoscroll: &Rc<SelectionAutoscroll>,
) {
    let area_weak = drawing_area.downgrade();
    let runtime_weak = Rc::downgrade(runtime);
    let selection_weak = Rc::downgrade(selection);
    let autoscroll_weak = Rc::downgrade(autoscroll);
    let renderer = renderer.clone();
    glib::timeout_add_local(SELECTION_AUTOSCROLL_INTERVAL, move || {
        let (Some(area), Some(runtime), Some(selection), Some(autoscroll)) = (
            area_weak.upgrade(),
            runtime_weak.upgrade(),
            selection_weak.upgrade(),
            autoscroll_weak.upgrade(),
        ) else {
            return glib::ControlFlow::Break;
        };
        let lines = autoscroll.lines.get();
        if lines == 0 || !selection.borrow().is_selecting() {
            autoscroll.active.set(false);
            return glib::ControlFlow::Break;
        }
        if let Err(err) = autoscroll_selection_tick(&runtime, &selection, lines) {
            eprintln!("Failed to autoscroll terminal selection: {err}");
            autoscroll.active.set(false);
            return glib::ControlFlow::Break;
        }
        // Keep the head pinned under the pointer, clamped into the viewport.
        let (x, y) = autoscroll.pointer.get();
        let max_y = (f64::from(area.allocated_height()) - 1.0).max(0.0);
        let cell = selection_cell_for_position(&area, &renderer, x, y.clamp(0.0, max_y));
        selection.borrow_mut().extend_drag(cell);
        area.queue_draw();
        glib::ControlFlow::Continue
    });
}

/// One drag-autoscroll step: scrolls the viewport by `lines` and re-anchors
/// the in-progress selection by however many rows the core actually scrolled
/// (the core clamps at the scrollback edges), so the highlight keeps covering
/// the same text. This is what tells autoscroll apart from a user wheel
/// scroll, which still clears the selection.
fn autoscroll_selection_tick(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    selection: &Rc<RefCell<TerminalSelection>>,
    lines: isize,
) -> Result<(), TerminalError> {
    // The runtime borrow is released before the selection is touched.
    let (scrolled_rows, max_row, max_col) = {
        let mut runtime = runtime.borrow_mut();
        let before = runtime.viewport_position()?;
        runtime.scroll_viewport_lines(lines)?;
        let after = runtime.viewport_position()?;
        let size = runtime.size();
        (
            after.top as isize - before.top as isize,
            after.rows.saturating_sub(1),
            usize::from(size.cols.saturating_sub(1)),
        )
    };
    if scrolled_rows != 0 {
        selection
            .borrow_mut()
            .compensate_scroll(scrolled_rows, max_row, max_col);
    }
    Ok(())
}

/// The text currently on screen, one line per viewport row, right-trimmed.
fn viewport_text_from_frame(frame: &forktty_terminal::ghostty::core::TerminalFrame) -> String {
    let text = frame
        .rows
        .iter()
        .map(|row| {
            row.cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    text.trim_end().to_string()
}

fn selection_text_from_frame(
    frame: &forktty_terminal::ghostty::core::TerminalFrame,
    start: SelectionPoint,
    end: SelectionPoint,
) -> String {
    let mut lines = Vec::new();
    for (row_idx, row) in frame.rows.iter().enumerate() {
        let Some((from, to)) = selection_cols_for_row(start, end, row_idx, row.cells.len()) else {
            continue;
        };
        let line: String = row.cells[from..to]
            .iter()
            .map(|cell| cell.text.as_str())
            .collect();
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScrollRouting {
    /// The event was encoded and written to a mouse-tracking application.
    Forwarded,
    /// Tracking is off; the viewport scrolled (or was already at its limit).
    ViewportScrolled(bool),
    /// Nothing to do (no line delta for this event).
    NotHandled,
}

/// Routes a wheel event: forwarded to a mouse-tracking application, otherwise
/// scrolled through the local viewport. Each runtime borrow is released
/// before the next one — matching directly on `borrow_mut().write_mouse(..)`
/// used to keep the `RefMut` alive across the arms, so the viewport-scroll
/// re-borrow panicked, and that panic aborted the whole app because it cannot
/// unwind across the GTK signal trampoline (wheel scroll over any pane with
/// mouse tracking off, e.g. a plain shell prompt).
fn route_terminal_scroll(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    input: TerminalMouseInput,
    dy: f64,
) -> Result<ScrollRouting, TerminalError> {
    let wrote = runtime.borrow_mut().write_mouse(input);
    if wrote? {
        return Ok(ScrollRouting::Forwarded);
    }
    let Some(delta) = terminal_scroll_viewport_delta(dy) else {
        return Ok(ScrollRouting::NotHandled);
    };
    let scrolled = runtime.borrow_mut().scroll_viewport_lines(delta);
    Ok(ScrollRouting::ViewportScrolled(scrolled?))
}

/// Snaps the viewport to the bottom before user input reaches the PTY
/// ("scroll on keystroke"). A finished selection is viewport-relative, so a
/// jump that actually moved drops it like a wheel scroll does.
fn kick_viewport_to_bottom(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    selection: &Rc<RefCell<TerminalSelection>>,
) -> Result<bool, TerminalError> {
    let scrolled = runtime.borrow_mut().scroll_viewport_to_bottom()?;
    if scrolled {
        selection.borrow_mut().clear();
    }
    Ok(scrolled)
}

/// Applies a Shift+PageUp/PageDown/Home/End scrollback navigation. Returns
/// `false` on the alternate screen, where there is no scrollback and the key
/// must keep going to the application; otherwise the key is consumed even at
/// the scrollback edges (it must not leak into the shell). A jump that moved
/// drops the viewport-relative selection, like a wheel scroll.
fn handle_scrollback_navigation(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    selection: &Rc<RefCell<TerminalSelection>>,
    navigation: ScrollbackNavigation,
) -> Result<bool, TerminalError> {
    if runtime.borrow().is_alternate_screen()? {
        return Ok(false);
    }
    let scrolled = {
        let mut runtime = runtime.borrow_mut();
        // One overlap row of context, the conventional terminal page step.
        let page = (runtime.size().rows.saturating_sub(1) as isize).max(1);
        match navigation {
            ScrollbackNavigation::PageUp => runtime.scroll_viewport_lines(-page)?,
            ScrollbackNavigation::PageDown => runtime.scroll_viewport_lines(page)?,
            ScrollbackNavigation::Top => runtime.scroll_viewport_to_top()?,
            ScrollbackNavigation::Bottom => runtime.scroll_viewport_to_bottom()?,
        }
    };
    if scrolled {
        selection.borrow_mut().clear();
    }
    Ok(true)
}

fn write_terminal_input(
    runtime: &Rc<RefCell<TerminalRuntime>>,
    selection: &Rc<RefCell<TerminalSelection>>,
    drawing_area: &gtk::DrawingArea,
    input: TerminalInput,
) {
    if let Err(err) = kick_viewport_to_bottom(runtime, selection) {
        eprintln!("Failed to scroll terminal to bottom: {err}");
    }
    let result = match input {
        TerminalInput::Bytes(bytes) => runtime.borrow_mut().write_bytes(&bytes),
        TerminalInput::Key(key) => runtime.borrow_mut().write_key(key),
    };
    if let Err(err) = result {
        eprintln!("Failed to write terminal key input: {err}");
    }
    drawing_area.queue_draw();
}

pub(super) trait TerminalWidgetOps {
    fn widget(&self) -> gtk::Widget;
    fn has_terminal_focus(&self) -> bool;
    fn write_input(&self, input: TerminalInput);
    fn grab_terminal_focus(&self);
    fn copy_text(&self);
    fn paste_from_clipboard(&self);
    fn select_all_text(&self);
    fn reset_and_clear(&self) {
        self.send_text("\x0c");
    }
    fn send_text(&self, text: &str);
    fn resize_cells(&self, cols: u16, rows: u16);
}

#[cfg(test)]
pub(super) fn copy_terminal_if_focused(widget: &impl TerminalWidgetOps) -> bool {
    if !widget.has_terminal_focus() {
        return false;
    }
    widget.copy_text();
    true
}

pub(super) fn forward_terminal_navigation_input(
    widget: &impl TerminalWidgetOps,
    input: TerminalInput,
) {
    widget.write_input(input);
    widget.grab_terminal_focus();
}

impl TerminalWidgetOps for GhosttyTerminalWidget {
    fn widget(&self) -> gtk::Widget {
        self.drawing_area.clone().upcast()
    }

    fn has_terminal_focus(&self) -> bool {
        self.drawing_area.has_focus()
    }

    fn write_input(&self, input: TerminalInput) {
        write_terminal_input(&self.runtime, &self.selection, &self.drawing_area, input);
    }

    fn grab_terminal_focus(&self) {
        self.drawing_area.grab_focus();
    }

    fn copy_text(&self) {
        if let Some(display) = gtk::gdk::Display::default() {
            // With nothing selected, copy what is on screen — not the whole
            // scrollback, which used to silently fill the clipboard with the
            // entire session history.
            let fallback = match self.runtime.borrow_mut().render_frame() {
                Ok(frame) => viewport_text_from_frame(&frame),
                Err(err) => {
                    eprintln!("Failed to render terminal frame for copy: {err}");
                    String::new()
                }
            };
            let text = copy_source_text(&self.selection.borrow(), &fallback);
            display.clipboard().set_text(&text);
        }
    }

    fn paste_from_clipboard(&self) {
        let runtime = self.runtime.clone();
        let selection = self.selection.clone();
        let drawing_area = self.drawing_area.clone();
        if let Some(display) = gtk::gdk::Display::default() {
            display
                .clipboard()
                .read_text_async(None::<&gio::Cancellable>, move |result| {
                    let Ok(Some(text)) = result else {
                        return;
                    };
                    if let Err(err) = kick_viewport_to_bottom(&runtime, &selection) {
                        eprintln!("Failed to scroll terminal to bottom: {err}");
                    }
                    if let Err(err) = runtime.borrow_mut().paste_text(text.as_str()) {
                        eprintln!("Failed to paste into terminal: {err}");
                    }
                    drawing_area.queue_draw();
                });
        }
    }

    fn select_all_text(&self) {
        {
            let mut selection = self.selection.borrow_mut();
            selection.clear();
            // Select-all covers the whole scrollback, like other terminals.
            selection.select_text(self.runtime.borrow().full_text());
        }
        self.copy_text();
    }

    fn reset_and_clear(&self) {
        self.selection.borrow_mut().clear();
        self.with_runtime(TerminalRuntime::reset_and_clear);
    }

    fn send_text(&self, text: &str) {
        self.with_runtime(|runtime| runtime.write_text(text));
    }

    fn resize_cells(&self, cols: u16, rows: u16) {
        self.with_runtime(|runtime| runtime.resize_cells(cols, rows));
    }
}

#[cfg(test)]
#[derive(Debug, Default)]
pub(super) struct TestTerminalWidget {
    sent_text: RefCell<Vec<String>>,
    inputs: RefCell<Vec<TerminalInput>>,
    focus_calls: Cell<usize>,
    calls: RefCell<Vec<String>>,
}

#[cfg(test)]
impl TestTerminalWidget {
    pub(super) fn sent_text(&self) -> Vec<String> {
        self.sent_text.borrow().clone()
    }

    pub(super) fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }

    pub(super) fn inputs(&self) -> Vec<TerminalInput> {
        self.inputs.borrow().clone()
    }

    pub(super) fn focus_calls(&self) -> usize {
        self.focus_calls.get()
    }
}

#[cfg(test)]
impl TerminalWidgetOps for TestTerminalWidget {
    fn widget(&self) -> gtk::Widget {
        panic!("test terminal widget has no GTK widget")
    }

    fn has_terminal_focus(&self) -> bool {
        true
    }

    fn write_input(&self, input: TerminalInput) {
        self.inputs.borrow_mut().push(input);
    }

    fn grab_terminal_focus(&self) {
        self.focus_calls.set(self.focus_calls.get() + 1);
    }

    fn copy_text(&self) {
        self.calls.borrow_mut().push("copy_text".to_string());
    }

    fn paste_from_clipboard(&self) {}

    fn select_all_text(&self) {}

    fn send_text(&self, text: &str) {
        self.sent_text.borrow_mut().push(text.to_string());
    }

    fn resize_cells(&self, _cols: u16, _rows: u16) {}
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    use forktty_terminal::ghostty::pty::PtySize;
    use std::path::PathBuf;

    fn frame_for_lines(lines: &[u8]) -> forktty_terminal::ghostty::core::TerminalFrame {
        let request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: vec!["-lc".to_string(), "sleep 10".to_string()],
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        };
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 20, rows: 4 }).unwrap();
        runtime.feed_pty_bytes(lines).unwrap();
        runtime.render_frame().unwrap()
    }

    #[test]
    fn selection_text_extracts_partial_first_and_last_rows() {
        let frame = frame_for_lines(b"alpha beta\r\ngamma delta\r\nepsilon");

        let text = selection_text_from_frame(
            &frame,
            SelectionPoint { row: 0, col: 6 },
            SelectionPoint { row: 2, col: 3 },
        );

        assert_eq!(text, "beta\ngamma delta\nepsi");
    }

    // Regression for the Fedora SIGABRT: wheel scroll over a pane with mouse
    // tracking off used to double-borrow the runtime RefCell (the match held
    // the write_mouse RefMut across the viewport-scroll re-borrow), and the
    // panic aborted the app because it cannot unwind across the GTK signal
    // trampoline.
    #[test]
    fn wheel_scroll_with_tracking_off_scrolls_the_viewport_without_panicking() {
        use forktty_terminal::ghostty::core::{
            TerminalMouseAction, TerminalMouseButton, TerminalMouseInput, TerminalMousePosition,
            TerminalMouseSize,
        };
        let request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: vec!["-lc".to_string(), "sleep 10".to_string()],
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        };
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 20, rows: 4 }).unwrap();
        runtime
            .feed_pty_bytes(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix")
            .unwrap();
        let runtime = Rc::new(RefCell::new(runtime));
        let wheel = TerminalMouseInput {
            action: TerminalMouseAction::Press,
            button: Some(TerminalMouseButton::WheelUp),
            modifiers: Default::default(),
            position: TerminalMousePosition { x: 10.0, y: 20.0 },
            size: TerminalMouseSize {
                screen_width: 800,
                screen_height: 480,
                cell_width: 10,
                cell_height: 20,
            },
            any_button_pressed: false,
        };

        let routing = route_terminal_scroll(&runtime, wheel, -1.0).unwrap();

        assert_eq!(routing, ScrollRouting::ViewportScrolled(true));
    }

    #[test]
    fn wheel_scroll_with_tracking_on_forwards_to_the_application() {
        use forktty_terminal::ghostty::core::{
            TerminalMouseAction, TerminalMouseButton, TerminalMouseInput, TerminalMousePosition,
            TerminalMouseSize,
        };
        let request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: vec!["-lc".to_string(), "sleep 10".to_string()],
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        };
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 20, rows: 4 }).unwrap();
        // Enable SGR mouse tracking, as tmux/vim/htop do.
        runtime.feed_pty_bytes(b"\x1b[?1000h\x1b[?1006h").unwrap();
        let runtime = Rc::new(RefCell::new(runtime));
        let wheel = TerminalMouseInput {
            action: TerminalMouseAction::Press,
            button: Some(TerminalMouseButton::WheelUp),
            modifiers: Default::default(),
            position: TerminalMousePosition { x: 10.0, y: 20.0 },
            size: TerminalMouseSize {
                screen_width: 800,
                screen_height: 480,
                cell_width: 10,
                cell_height: 20,
            },
            any_button_pressed: false,
        };

        let routing = route_terminal_scroll(&runtime, wheel, -1.0).unwrap();

        assert_eq!(routing, ScrollRouting::Forwarded);
    }

    #[test]
    fn word_selection_expands_a_path_under_the_click() {
        let frame = frame_for_lines(b"cd /tmp/x.txt now");

        assert_eq!(
            word_selection_in_frame(&frame, SelectionPoint { row: 0, col: 5 }),
            Some((
                SelectionPoint { row: 0, col: 3 },
                SelectionPoint { row: 0, col: 12 }
            ))
        );
        // Unwritten cells (row 2 is blank on the 4-row screen) select nothing.
        assert_eq!(
            word_selection_in_frame(&frame, SelectionPoint { row: 2, col: 0 }),
            None
        );
        // A click below the frame selects nothing.
        assert_eq!(
            word_selection_in_frame(&frame, SelectionPoint { row: 9, col: 0 }),
            None
        );
    }

    #[test]
    fn word_selection_includes_wide_cells_and_their_spacers() {
        // "漢" and "字" each occupy a wide cell plus a spacer tail (cols 2-5).
        let frame = frame_for_lines("a \u{6f22}\u{5b57} b".as_bytes());

        // Clicking the spacer tail behaves like clicking the wide head.
        assert_eq!(
            word_selection_in_frame(&frame, SelectionPoint { row: 0, col: 3 }),
            Some((
                SelectionPoint { row: 0, col: 2 },
                SelectionPoint { row: 0, col: 5 }
            ))
        );
    }

    #[test]
    fn line_selection_covers_the_whole_visual_row() {
        let frame = frame_for_lines(b"alpha beta\r\ngamma");
        let cols = frame.rows[0].cells.len();

        assert_eq!(
            line_selection_in_frame(&frame, 1),
            Some((
                SelectionPoint { row: 1, col: 0 },
                SelectionPoint {
                    row: 1,
                    col: cols - 1
                }
            ))
        );
        assert_eq!(line_selection_in_frame(&frame, 9), None);
    }

    #[test]
    fn commit_selection_range_stores_text_and_clears_on_blank() {
        let frame = frame_for_lines(b"alpha beta\r\ngamma");
        let mut selection = TerminalSelection::default();

        commit_selection_range(
            &mut selection,
            &frame,
            SelectionPoint { row: 0, col: 6 },
            SelectionPoint { row: 0, col: 9 },
        );
        assert_eq!(copy_source_text(&selection, "fallback"), "beta");
        assert!(!selection.is_selecting());
        assert!(selection.normalized_range().is_some());

        // A blank row leaves no selection behind.
        commit_selection_range(
            &mut selection,
            &frame,
            SelectionPoint { row: 3, col: 0 },
            SelectionPoint { row: 3, col: 5 },
        );
        assert_eq!(copy_source_text(&selection, "fallback"), "fallback");
        assert_eq!(selection.normalized_range(), None);
    }

    #[test]
    fn autoscroll_tick_scrolls_and_reanchors_the_drag_selection() {
        let request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: vec!["-lc".to_string(), "sleep 10".to_string()],
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        };
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 20, rows: 4 }).unwrap();
        // 6 lines on a 4-row screen: 2 rows of scrollback, viewport at the
        // bottom (top = 2).
        runtime
            .feed_pty_bytes(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix")
            .unwrap();
        let runtime = Rc::new(RefCell::new(runtime));
        let selection = Rc::new(RefCell::new(TerminalSelection::default()));
        selection
            .borrow_mut()
            .begin_drag(SelectionPoint { row: 1, col: 0 });
        selection
            .borrow_mut()
            .extend_drag(SelectionPoint { row: 2, col: 3 });

        // Scrolling up by 2 moves the selected content down by 2 viewport
        // rows; the head falls off the bottom and clamps there.
        autoscroll_selection_tick(&runtime, &selection, -2).unwrap();

        assert_eq!(runtime.borrow().viewport_position().unwrap().top, 0);
        assert_eq!(
            selection.borrow().normalized_range(),
            Some((
                SelectionPoint { row: 3, col: 0 },
                SelectionPoint { row: 3, col: 19 }
            ))
        );
        assert!(selection.borrow().is_selecting());

        // Already at the top: the core clamps the scroll to zero rows and the
        // selection must not move.
        autoscroll_selection_tick(&runtime, &selection, -2).unwrap();

        assert_eq!(
            selection.borrow().normalized_range(),
            Some((
                SelectionPoint { row: 3, col: 0 },
                SelectionPoint { row: 3, col: 19 }
            ))
        );
    }

    #[test]
    fn viewport_text_covers_only_the_visible_frame() {
        // 4 rows, so "alpha beta\ngamma" fills rows 0-1 and leaves 2-3 blank;
        // the copy fallback must trim those rather than dump scrollback.
        let frame = frame_for_lines(b"alpha beta\r\ngamma");

        assert_eq!(viewport_text_from_frame(&frame), "alpha beta\ngamma");
    }

    #[test]
    fn selection_text_single_row_is_inclusive_of_end_cell() {
        let frame = frame_for_lines(b"alpha beta");

        let text = selection_text_from_frame(
            &frame,
            SelectionPoint { row: 0, col: 0 },
            SelectionPoint { row: 0, col: 4 },
        );

        assert_eq!(text, "alpha");
    }

    #[test]
    fn scrollback_navigation_pages_and_jumps_outside_the_alternate_screen() {
        let request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: vec!["-lc".to_string(), "sleep 10".to_string()],
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        };
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 20, rows: 4 }).unwrap();
        let lines: String = (1..=20).map(|i| format!("line{i}\r\n")).collect();
        runtime.feed_pty_bytes(lines.as_bytes()).unwrap();
        let runtime = Rc::new(RefCell::new(runtime));
        let selection = Rc::new(RefCell::new(TerminalSelection::default()));

        let bottom = runtime.borrow().viewport_position().unwrap().top;
        // PageUp scrolls one page minus one overlap row and consumes the key.
        assert!(
            handle_scrollback_navigation(&runtime, &selection, ScrollbackNavigation::PageUp)
                .unwrap()
        );
        assert_eq!(
            runtime.borrow().viewport_position().unwrap().top,
            bottom - 3
        );
        assert!(
            handle_scrollback_navigation(&runtime, &selection, ScrollbackNavigation::Top).unwrap()
        );
        assert_eq!(runtime.borrow().viewport_position().unwrap().top, 0);
        // At the top, PageUp still consumes the key (it must not type into the
        // shell) even though nothing moves.
        assert!(
            handle_scrollback_navigation(&runtime, &selection, ScrollbackNavigation::PageUp)
                .unwrap()
        );
        assert!(
            handle_scrollback_navigation(&runtime, &selection, ScrollbackNavigation::Bottom)
                .unwrap()
        );
        let viewport = runtime.borrow().viewport_position().unwrap();
        assert_eq!(viewport.top + viewport.rows, viewport.total);

        // On the alternate screen the key is NOT consumed: it belongs to the app.
        runtime.borrow_mut().feed_pty_bytes(b"\x1b[?1049h").unwrap();
        assert!(
            !handle_scrollback_navigation(&runtime, &selection, ScrollbackNavigation::PageUp)
                .unwrap()
        );
    }

    #[test]
    fn kick_viewport_scrolls_to_bottom_and_clears_the_selection() {
        let request = SpawnRequest {
            surface_id: "surface-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            shell: "/bin/sh".to_string(),
            args: vec!["-lc".to_string(), "sleep 10".to_string()],
            cwd: PathBuf::from("/tmp"),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
        };
        let mut runtime = TerminalRuntime::spawn(&request, PtySize { cols: 20, rows: 4 }).unwrap();
        runtime
            .feed_pty_bytes(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix")
            .unwrap();
        runtime.scroll_viewport_lines(-2).unwrap();
        let runtime = Rc::new(RefCell::new(runtime));
        let selection = Rc::new(RefCell::new(TerminalSelection::default()));
        selection.borrow_mut().select_text("stale");
        selection
            .borrow_mut()
            .begin_drag(SelectionPoint { row: 0, col: 0 });
        selection
            .borrow_mut()
            .extend_drag(SelectionPoint { row: 0, col: 3 });
        selection.borrow_mut().end_drag();

        kick_viewport_to_bottom(&runtime, &selection).unwrap();

        let viewport = runtime.borrow().viewport_position().unwrap();
        assert_eq!(viewport.top + viewport.rows, viewport.total);
        // The selection was viewport-relative; after the jump it would highlight
        // the wrong text.
        assert_eq!(selection.borrow().normalized_range(), None);

        // At the bottom already: a second kick must not clear a fresh selection.
        selection
            .borrow_mut()
            .begin_drag(SelectionPoint { row: 0, col: 0 });
        selection
            .borrow_mut()
            .extend_drag(SelectionPoint { row: 0, col: 3 });
        selection.borrow_mut().end_drag();
        kick_viewport_to_bottom(&runtime, &selection).unwrap();
        assert!(selection.borrow().normalized_range().is_some());
    }
}

#[cfg(test)]
mod mouse_tests {
    use super::*;
    use forktty_terminal::ghostty::core::{
        TerminalKeyModifiers, TerminalMouseAction, TerminalMouseButton, TerminalMousePosition,
        TerminalMouseSize,
    };

    #[test]
    fn terminal_mouse_input_builds_core_input_from_widget_metrics() {
        let modifiers = gtk::gdk::ModifierType::SHIFT_MASK | gtk::gdk::ModifierType::ALT_MASK;

        let input = terminal_mouse_input(
            TerminalMouseEventInput {
                action: TerminalMouseAction::Press,
                button: Some(TerminalMouseButton::Left),
                modifiers,
                x: 12.5,
                y: 24.0,
                any_button_pressed: true,
            },
            TerminalMouseWidgetMetrics {
                screen_width: 800,
                screen_height: 480,
                cell_width: 10,
                cell_height: 20,
            },
        );

        assert_eq!(input.action, TerminalMouseAction::Press);
        assert_eq!(input.button, Some(TerminalMouseButton::Left));
        assert_eq!(
            input.modifiers,
            TerminalKeyModifiers {
                shift: true,
                alt: true,
                ctrl: false,
            }
        );
        assert_eq!(input.position, TerminalMousePosition { x: 12.5, y: 24.0 });
        assert_eq!(
            input.size,
            TerminalMouseSize {
                screen_width: 800,
                screen_height: 480,
                cell_width: 10,
                cell_height: 20,
            }
        );
        assert!(input.any_button_pressed);
    }
}
