use super::*;
use forktty_terminal::ghostty::core::{
    TerminalKey, TerminalKeyInput, TerminalKeyModifiers, TerminalMouseButton,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GhosttyKeySpec {
    key: GhosttyKey,
    ctrl: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GhosttyKey {
    Enter,
    Char(char),
}

impl GhosttyKeySpec {
    #[cfg(test)]
    pub(super) fn enter() -> Self {
        Self {
            key: GhosttyKey::Enter,
            ctrl: false,
        }
    }

    #[cfg(test)]
    pub(super) fn ctrl(ch: char) -> Self {
        Self {
            key: GhosttyKey::Char(ch),
            ctrl: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TerminalInput {
    Bytes(Vec<u8>),
    Key(TerminalKeyInput),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TerminalNavigationFallbackFocus {
    TerminalSurface,
    AppChrome,
    TextInput,
    Popover,
}

pub(super) fn terminal_text_input(text: &str) -> Option<TerminalInput> {
    if text.is_empty() {
        None
    } else {
        Some(TerminalInput::Bytes(text.as_bytes().to_vec()))
    }
}

pub(super) fn translate_gtk_key(
    key: gtk::gdk::Key,
    modifiers: gtk::gdk::ModifierType,
    text: Option<&str>,
) -> Option<TerminalInput> {
    if is_forktty_accelerator(key, modifiers) {
        return None;
    }
    let ctrl = modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
    let alt = modifiers.contains(gtk::gdk::ModifierType::ALT_MASK);
    let shift = modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK);
    let terminal_modifiers = terminal_key_modifiers(modifiers);
    let spec = match key {
        gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter => GhosttyKeySpec {
            key: GhosttyKey::Enter,
            ctrl,
        },
        gtk::gdk::Key::BackSpace => {
            return Some(TerminalInput::Bytes(if alt {
                vec![0x1b, 0x7f]
            } else {
                vec![0x7f]
            }));
        }
        gtk::gdk::Key::Tab => {
            return Some(TerminalInput::Bytes(if shift {
                b"\x1b[Z".to_vec()
            } else {
                b"\t".to_vec()
            }));
        }
        gtk::gdk::Key::ISO_Left_Tab => return Some(TerminalInput::Bytes(b"\x1b[Z".to_vec())),
        gtk::gdk::Key::Escape => return Some(TerminalInput::Bytes(b"\x1b".to_vec())),
        gtk::gdk::Key::Up | gtk::gdk::Key::KP_Up => {
            return Some(terminal_key_input(TerminalKey::ArrowUp, terminal_modifiers));
        }
        gtk::gdk::Key::Down | gtk::gdk::Key::KP_Down => {
            return Some(terminal_key_input(
                TerminalKey::ArrowDown,
                terminal_modifiers,
            ));
        }
        gtk::gdk::Key::Right | gtk::gdk::Key::KP_Right => {
            return Some(terminal_key_input(
                TerminalKey::ArrowRight,
                terminal_modifiers,
            ));
        }
        gtk::gdk::Key::Left | gtk::gdk::Key::KP_Left => {
            return Some(terminal_key_input(
                TerminalKey::ArrowLeft,
                terminal_modifiers,
            ));
        }
        gtk::gdk::Key::Home | gtk::gdk::Key::KP_Home => {
            return Some(terminal_key_input(TerminalKey::Home, terminal_modifiers));
        }
        gtk::gdk::Key::End | gtk::gdk::Key::KP_End => {
            return Some(terminal_key_input(TerminalKey::End, terminal_modifiers));
        }
        gtk::gdk::Key::Page_Up | gtk::gdk::Key::KP_Page_Up => {
            return Some(terminal_key_input(TerminalKey::PageUp, terminal_modifiers));
        }
        gtk::gdk::Key::Page_Down | gtk::gdk::Key::KP_Page_Down => {
            return Some(terminal_key_input(
                TerminalKey::PageDown,
                terminal_modifiers,
            ));
        }
        gtk::gdk::Key::Insert | gtk::gdk::Key::KP_Insert => {
            return Some(terminal_key_input(TerminalKey::Insert, terminal_modifiers));
        }
        gtk::gdk::Key::Delete | gtk::gdk::Key::KP_Delete => {
            return Some(terminal_key_input(TerminalKey::Delete, terminal_modifiers));
        }
        _ => {
            if !ctrl || alt {
                if let Some(text) = text.filter(|text| !text.is_empty()) {
                    return Some(TerminalInput::Bytes(text.as_bytes().to_vec()));
                }
            }
            if let Some(ch) = key.to_unicode() {
                GhosttyKeySpec {
                    key: GhosttyKey::Char(ch),
                    ctrl,
                }
            } else {
                return None;
            }
        }
    };
    encode_key(spec).map(TerminalInput::Bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScrollbackNavigation {
    PageUp,
    PageDown,
    Top,
    Bottom,
}

/// Maps Shift+PageUp/PageDown/Home/End to local scrollback navigation.
/// Exactly Shift: with Ctrl or Alt the combination stays an application key
/// or an existing forktty accelerator.
pub(super) fn scrollback_navigation_for_key(
    key: gtk::gdk::Key,
    modifiers: gtk::gdk::ModifierType,
) -> Option<ScrollbackNavigation> {
    let shift = modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK);
    let ctrl = modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
    let alt = modifiers.contains(gtk::gdk::ModifierType::ALT_MASK);
    if !shift || ctrl || alt {
        return None;
    }
    match key {
        gtk::gdk::Key::Page_Up | gtk::gdk::Key::KP_Page_Up => Some(ScrollbackNavigation::PageUp),
        gtk::gdk::Key::Page_Down | gtk::gdk::Key::KP_Page_Down => {
            Some(ScrollbackNavigation::PageDown)
        }
        gtk::gdk::Key::Home | gtk::gdk::Key::KP_Home => Some(ScrollbackNavigation::Top),
        gtk::gdk::Key::End | gtk::gdk::Key::KP_End => Some(ScrollbackNavigation::Bottom),
        _ => None,
    }
}

pub(super) fn translate_gtk_navigation_key(
    key: gtk::gdk::Key,
    modifiers: gtk::gdk::ModifierType,
) -> Option<TerminalInput> {
    if !is_terminal_navigation_key(key) {
        return None;
    }
    translate_gtk_key(key, modifiers, None)
}

pub(super) fn terminal_navigation_fallback_focus(
    focus: Option<&gtk::Widget>,
) -> TerminalNavigationFallbackFocus {
    let Some(focus) = focus else {
        return TerminalNavigationFallbackFocus::AppChrome;
    };
    if focus.ancestor(gtk::Popover::static_type()).is_some() {
        return TerminalNavigationFallbackFocus::Popover;
    }
    if focused_widget_is_text_input(focus) {
        return TerminalNavigationFallbackFocus::TextInput;
    }
    if focus.has_css_class("ghostty-terminal") {
        return TerminalNavigationFallbackFocus::TerminalSurface;
    }
    TerminalNavigationFallbackFocus::AppChrome
}

pub(super) fn terminal_navigation_fallback_allowed(focus: TerminalNavigationFallbackFocus) -> bool {
    matches!(
        focus,
        TerminalNavigationFallbackFocus::AppChrome
            | TerminalNavigationFallbackFocus::TerminalSurface
    )
}

fn focused_widget_is_text_input(widget: &gtk::Widget) -> bool {
    widget.is::<gtk::Entry>()
        || widget.is::<gtk::SearchEntry>()
        || widget.is::<gtk::SpinButton>()
        || widget.is::<gtk::TextView>()
}

fn is_terminal_navigation_key(key: gtk::gdk::Key) -> bool {
    matches!(
        key,
        gtk::gdk::Key::Up
            | gtk::gdk::Key::KP_Up
            | gtk::gdk::Key::Down
            | gtk::gdk::Key::KP_Down
            | gtk::gdk::Key::Right
            | gtk::gdk::Key::KP_Right
            | gtk::gdk::Key::Left
            | gtk::gdk::Key::KP_Left
            | gtk::gdk::Key::Home
            | gtk::gdk::Key::KP_Home
            | gtk::gdk::Key::End
            | gtk::gdk::Key::KP_End
            | gtk::gdk::Key::Page_Up
            | gtk::gdk::Key::KP_Page_Up
            | gtk::gdk::Key::Page_Down
            | gtk::gdk::Key::KP_Page_Down
            | gtk::gdk::Key::Insert
            | gtk::gdk::Key::KP_Insert
            | gtk::gdk::Key::Delete
            | gtk::gdk::Key::KP_Delete
    )
}

fn encode_key(spec: GhosttyKeySpec) -> Option<Vec<u8>> {
    match spec.key {
        GhosttyKey::Enter => Some(b"\r".to_vec()),
        GhosttyKey::Char(ch) if spec.ctrl => control_code(ch).map(|byte| vec![byte]),
        GhosttyKey::Char(ch) => {
            let mut bytes = [0; 4];
            Some(ch.encode_utf8(&mut bytes).as_bytes().to_vec())
        }
    }
}

fn terminal_key_input(key: TerminalKey, modifiers: TerminalKeyModifiers) -> TerminalInput {
    TerminalInput::Key(TerminalKeyInput::new(key).with_modifiers(modifiers))
}

pub(super) fn terminal_key_modifiers(modifiers: gtk::gdk::ModifierType) -> TerminalKeyModifiers {
    TerminalKeyModifiers {
        shift: modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK),
        alt: modifiers.contains(gtk::gdk::ModifierType::ALT_MASK),
        ctrl: modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK),
    }
}

pub(super) fn terminal_mouse_button(button: u32) -> Option<TerminalMouseButton> {
    match button {
        gtk::gdk::BUTTON_PRIMARY => Some(TerminalMouseButton::Left),
        gtk::gdk::BUTTON_SECONDARY => Some(TerminalMouseButton::Right),
        gtk::gdk::BUTTON_MIDDLE => Some(TerminalMouseButton::Middle),
        _ => None,
    }
}

pub(super) fn terminal_scroll_button(delta_y: f64) -> Option<TerminalMouseButton> {
    if delta_y < 0.0 {
        Some(TerminalMouseButton::WheelUp)
    } else if delta_y > 0.0 {
        Some(TerminalMouseButton::WheelDown)
    } else {
        None
    }
}

fn control_code(ch: char) -> Option<u8> {
    let lower = ch.to_ascii_lowercase();
    if lower.is_ascii_lowercase() {
        return Some((lower as u8) - b'a' + 1);
    }
    // Standard C0 mappings for Ctrl with non-letter keys.
    match ch {
        ' ' | '@' => Some(0x00),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        '?' => Some(0x7f),
        _ => None,
    }
}

fn is_forktty_accelerator(key: gtk::gdk::Key, modifiers: gtk::gdk::ModifierType) -> bool {
    let ctrl = modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
    let alt = modifiers.contains(gtk::gdk::ModifierType::ALT_MASK);
    let shift = modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK);
    let ctrl_shift_app_action = ctrl
        && shift
        && !alt
        && matches!(
            key,
            gtk::gdk::Key::A
                | gtk::gdk::Key::a
                | gtk::gdk::Key::C
                | gtk::gdk::Key::c
                | gtk::gdk::Key::E
                | gtk::gdk::Key::e
                | gtk::gdk::Key::H
                | gtk::gdk::Key::h
                | gtk::gdk::Key::M
                | gtk::gdk::Key::m
                | gtk::gdk::Key::N
                | gtk::gdk::Key::n
                | gtk::gdk::Key::O
                | gtk::gdk::Key::o
                | gtk::gdk::Key::P
                | gtk::gdk::Key::p
                | gtk::gdk::Key::R
                | gtk::gdk::Key::r
                | gtk::gdk::Key::T
                | gtk::gdk::Key::t
                | gtk::gdk::Key::V
                | gtk::gdk::Key::v
                | gtk::gdk::Key::W
                | gtk::gdk::Key::w
                | gtk::gdk::Key::Return
                | gtk::gdk::Key::KP_Enter
        );
    let ctrl_app_action =
        ctrl && !shift && !alt && matches!(key, gtk::gdk::Key::B | gtk::gdk::Key::b);
    let tab_navigation_action = ctrl
        && !shift
        && !alt
        && matches!(
            key,
            gtk::gdk::Key::Page_Up
                | gtk::gdk::Key::KP_Page_Up
                | gtk::gdk::Key::Page_Down
                | gtk::gdk::Key::KP_Page_Down
                | gtk::gdk::Key::Home
                | gtk::gdk::Key::KP_Home
                | gtk::gdk::Key::End
                | gtk::gdk::Key::KP_End
        );
    ctrl_shift_app_action || ctrl_app_action || tab_navigation_action
}

#[cfg(test)]
fn encode_test_key(spec: GhosttyKeySpec) -> Option<Vec<u8>> {
    encode_key(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_translation_encodes_enter_and_ctrl_c() {
        assert_eq!(encode_test_key(GhosttyKeySpec::enter()).unwrap(), b"\r");
        assert_eq!(encode_test_key(GhosttyKeySpec::ctrl('c')).unwrap(), b"\x03");
    }

    #[test]
    fn key_translation_encodes_ctrl_with_non_letter_keys() {
        assert_eq!(encode_test_key(GhosttyKeySpec::ctrl(' ')).unwrap(), b"\x00");
        assert_eq!(encode_test_key(GhosttyKeySpec::ctrl('[')).unwrap(), b"\x1b");
        assert_eq!(
            encode_test_key(GhosttyKeySpec::ctrl('\\')).unwrap(),
            b"\x1c"
        );
        assert_eq!(encode_test_key(GhosttyKeySpec::ctrl('_')).unwrap(), b"\x1f");

        // The GTK key path must reach the same C0 codes with Ctrl held.
        let ctrl = gtk::gdk::ModifierType::CONTROL_MASK;
        assert_eq!(
            translate_gtk_key(gtk::gdk::Key::space, ctrl, Some(" ")).unwrap(),
            TerminalInput::Bytes(b"\x00".to_vec())
        );
        assert_eq!(
            translate_gtk_key(gtk::gdk::Key::bracketleft, ctrl, Some("[")).unwrap(),
            TerminalInput::Bytes(b"\x1b".to_vec())
        );
        assert_eq!(
            translate_gtk_key(gtk::gdk::Key::backslash, ctrl, Some("\\")).unwrap(),
            TerminalInput::Bytes(b"\x1c".to_vec())
        );
        assert_eq!(
            translate_gtk_key(gtk::gdk::Key::underscore, ctrl, Some("_")).unwrap(),
            TerminalInput::Bytes(b"\x1f".to_vec())
        );
    }

    #[test]
    fn key_translation_prefers_event_text_for_printable_input() {
        let none = gtk::gdk::ModifierType::empty();

        assert_eq!(
            translate_gtk_key(gtk::gdk::Key::a, none, Some("à")).unwrap(),
            TerminalInput::Bytes("à".as_bytes().to_vec())
        );
    }

    #[test]
    fn terminal_text_input_encodes_non_empty_utf8_commits() {
        assert_eq!(
            terminal_text_input("à").unwrap(),
            TerminalInput::Bytes("à".as_bytes().to_vec())
        );
        assert!(terminal_text_input("").is_none());
    }

    #[test]
    fn key_translation_keeps_control_codes_ahead_of_event_text() {
        let ctrl = gtk::gdk::ModifierType::CONTROL_MASK;

        assert_eq!(
            translate_gtk_key(gtk::gdk::Key::c, ctrl, Some("c")).unwrap(),
            TerminalInput::Bytes(b"\x03".to_vec())
        );
    }

    #[test]
    fn key_translation_treats_ctrl_alt_event_text_as_composed_input() {
        let altgr_like = gtk::gdk::ModifierType::CONTROL_MASK | gtk::gdk::ModifierType::ALT_MASK;

        assert_eq!(
            translate_gtk_key(gtk::gdk::Key::e, altgr_like, Some("€")).unwrap(),
            TerminalInput::Bytes("€".as_bytes().to_vec())
        );
    }

    #[test]
    fn key_translation_routes_arrow_keys_through_terminal_core() {
        use forktty_terminal::ghostty::core::{TerminalKey, TerminalKeyInput};

        let none = gtk::gdk::ModifierType::empty();

        assert_eq!(
            translate_gtk_key(gtk::gdk::Key::Up, none, None).unwrap(),
            TerminalInput::Key(TerminalKeyInput::new(TerminalKey::ArrowUp))
        );
        assert_eq!(
            translate_gtk_key(gtk::gdk::Key::Down, none, None).unwrap(),
            TerminalInput::Key(TerminalKeyInput::new(TerminalKey::ArrowDown))
        );
        assert_eq!(
            translate_gtk_key(gtk::gdk::Key::Right, none, None).unwrap(),
            TerminalInput::Key(TerminalKeyInput::new(TerminalKey::ArrowRight))
        );
        assert_eq!(
            translate_gtk_key(gtk::gdk::Key::Left, none, None).unwrap(),
            TerminalInput::Key(TerminalKeyInput::new(TerminalKey::ArrowLeft))
        );
    }

    #[test]
    fn key_translation_routes_navigation_keys_through_terminal_core() {
        use forktty_terminal::ghostty::core::{TerminalKey, TerminalKeyInput};

        let none = gtk::gdk::ModifierType::empty();

        for (gtk_key, terminal_key) in [
            (gtk::gdk::Key::Home, TerminalKey::Home),
            (gtk::gdk::Key::KP_Home, TerminalKey::Home),
            (gtk::gdk::Key::End, TerminalKey::End),
            (gtk::gdk::Key::KP_End, TerminalKey::End),
            (gtk::gdk::Key::Page_Up, TerminalKey::PageUp),
            (gtk::gdk::Key::KP_Page_Up, TerminalKey::PageUp),
            (gtk::gdk::Key::Page_Down, TerminalKey::PageDown),
            (gtk::gdk::Key::KP_Page_Down, TerminalKey::PageDown),
            (gtk::gdk::Key::Insert, TerminalKey::Insert),
            (gtk::gdk::Key::KP_Insert, TerminalKey::Insert),
            (gtk::gdk::Key::Delete, TerminalKey::Delete),
            (gtk::gdk::Key::KP_Delete, TerminalKey::Delete),
        ] {
            assert_eq!(
                translate_gtk_key(gtk_key, none, None).unwrap(),
                TerminalInput::Key(TerminalKeyInput::new(terminal_key)),
                "translated {gtk_key:?}"
            );
        }
    }

    #[test]
    fn key_translation_preserves_terminal_key_modifiers() {
        use forktty_terminal::ghostty::core::{
            TerminalKey, TerminalKeyInput, TerminalKeyModifiers,
        };

        let modifiers = gtk::gdk::ModifierType::SHIFT_MASK | gtk::gdk::ModifierType::ALT_MASK;

        assert_eq!(
            translate_gtk_key(gtk::gdk::Key::Up, modifiers, None).unwrap(),
            TerminalInput::Key(TerminalKeyInput::new(TerminalKey::ArrowUp).with_modifiers(
                TerminalKeyModifiers {
                    shift: true,
                    alt: true,
                    ctrl: false,
                }
            ))
        );
    }

    #[test]
    fn key_translation_routes_ctrl_alt_arrows_through_terminal_core() {
        use forktty_terminal::ghostty::core::{
            TerminalKey, TerminalKeyInput, TerminalKeyModifiers,
        };

        let pane_shortcut = gtk::gdk::ModifierType::CONTROL_MASK | gtk::gdk::ModifierType::ALT_MASK;

        assert_eq!(
            translate_gtk_key(gtk::gdk::Key::Left, pane_shortcut, None).unwrap(),
            TerminalInput::Key(
                TerminalKeyInput::new(TerminalKey::ArrowLeft).with_modifiers(
                    TerminalKeyModifiers {
                        shift: false,
                        alt: true,
                        ctrl: true,
                    }
                )
            )
        );
        assert_eq!(
            translate_gtk_key(gtk::gdk::Key::Right, pane_shortcut, None).unwrap(),
            TerminalInput::Key(
                TerminalKeyInput::new(TerminalKey::ArrowRight).with_modifiers(
                    TerminalKeyModifiers {
                        shift: false,
                        alt: true,
                        ctrl: true,
                    }
                )
            )
        );
    }

    #[test]
    fn pane_navigation_fallback_routes_only_terminal_navigation_keys() {
        use forktty_terminal::ghostty::core::{TerminalKey, TerminalKeyInput};

        let none = gtk::gdk::ModifierType::empty();
        let ctrl = gtk::gdk::ModifierType::CONTROL_MASK;

        assert_eq!(
            translate_gtk_navigation_key(gtk::gdk::Key::Up, none).unwrap(),
            TerminalInput::Key(TerminalKeyInput::new(TerminalKey::ArrowUp))
        );
        assert_eq!(
            translate_gtk_navigation_key(gtk::gdk::Key::KP_Left, none).unwrap(),
            TerminalInput::Key(TerminalKeyInput::new(TerminalKey::ArrowLeft))
        );
        assert!(translate_gtk_navigation_key(gtk::gdk::Key::a, none).is_none());
        assert!(translate_gtk_navigation_key(gtk::gdk::Key::Page_Up, ctrl).is_none());
    }

    #[test]
    fn mouse_button_translation_maps_gtk_buttons_to_terminal_buttons() {
        use forktty_terminal::ghostty::core::TerminalMouseButton;

        assert_eq!(
            terminal_mouse_button(gtk::gdk::BUTTON_PRIMARY),
            Some(TerminalMouseButton::Left)
        );
        assert_eq!(
            terminal_mouse_button(gtk::gdk::BUTTON_MIDDLE),
            Some(TerminalMouseButton::Middle)
        );
        assert_eq!(
            terminal_mouse_button(gtk::gdk::BUTTON_SECONDARY),
            Some(TerminalMouseButton::Right)
        );
        assert_eq!(terminal_mouse_button(8), None);
    }

    #[test]
    fn mouse_scroll_translation_maps_vertical_delta_to_wheel_buttons() {
        use forktty_terminal::ghostty::core::TerminalMouseButton;

        assert_eq!(
            terminal_scroll_button(-1.0),
            Some(TerminalMouseButton::WheelUp)
        );
        assert_eq!(
            terminal_scroll_button(1.0),
            Some(TerminalMouseButton::WheelDown)
        );
        assert_eq!(terminal_scroll_button(0.0), None);
    }

    #[test]
    fn key_translation_leaves_app_shortcuts_for_actions() {
        let ctrl_shift = gtk::gdk::ModifierType::CONTROL_MASK | gtk::gdk::ModifierType::SHIFT_MASK;
        let ctrl = gtk::gdk::ModifierType::CONTROL_MASK;

        assert!(translate_gtk_key(gtk::gdk::Key::C, ctrl_shift, None).is_none());
        assert!(translate_gtk_key(gtk::gdk::Key::V, ctrl_shift, None).is_none());
        assert!(translate_gtk_key(gtk::gdk::Key::A, ctrl_shift, None).is_none());
        assert!(translate_gtk_key(gtk::gdk::Key::Return, ctrl_shift, None).is_none());
        assert!(translate_gtk_key(gtk::gdk::Key::b, ctrl, None).is_none());
    }

    #[test]
    fn key_translation_leaves_tab_navigation_shortcuts_for_actions() {
        let ctrl = gtk::gdk::ModifierType::CONTROL_MASK;

        assert!(translate_gtk_key(gtk::gdk::Key::Page_Up, ctrl, None).is_none());
        assert!(translate_gtk_key(gtk::gdk::Key::Page_Down, ctrl, None).is_none());
        assert!(translate_gtk_key(gtk::gdk::Key::Home, ctrl, None).is_none());
        assert!(translate_gtk_key(gtk::gdk::Key::End, ctrl, None).is_none());
        assert!(translate_gtk_key(gtk::gdk::Key::KP_Page_Up, ctrl, None).is_none());
        assert!(translate_gtk_key(gtk::gdk::Key::KP_Page_Down, ctrl, None).is_none());
        assert!(translate_gtk_key(gtk::gdk::Key::KP_Home, ctrl, None).is_none());
        assert!(translate_gtk_key(gtk::gdk::Key::KP_End, ctrl, None).is_none());
    }

    #[test]
    fn shift_tab_sends_backtab_escape_sequence() {
        let shift = gtk::gdk::ModifierType::SHIFT_MASK;
        assert_eq!(
            translate_gtk_key(gtk::gdk::Key::Tab, shift, None).unwrap(),
            TerminalInput::Bytes(b"\x1b[Z".to_vec())
        );
        assert_eq!(
            translate_gtk_key(gtk::gdk::Key::ISO_Left_Tab, shift, None).unwrap(),
            TerminalInput::Bytes(b"\x1b[Z".to_vec())
        );
    }

    #[test]
    fn plain_tab_sends_horizontal_tab() {
        let none = gtk::gdk::ModifierType::empty();
        assert_eq!(
            translate_gtk_key(gtk::gdk::Key::Tab, none, None).unwrap(),
            TerminalInput::Bytes(b"\t".to_vec())
        );
    }

    #[test]
    fn alt_backspace_sends_meta_delete() {
        let alt = gtk::gdk::ModifierType::ALT_MASK;
        assert_eq!(
            translate_gtk_key(gtk::gdk::Key::BackSpace, alt, None).unwrap(),
            TerminalInput::Bytes(b"\x1b\x7f".to_vec())
        );
    }

    #[test]
    fn plain_backspace_sends_delete() {
        let none = gtk::gdk::ModifierType::empty();
        assert_eq!(
            translate_gtk_key(gtk::gdk::Key::BackSpace, none, None).unwrap(),
            TerminalInput::Bytes(vec![0x7f])
        );
    }

    #[test]
    fn shift_navigation_keys_map_to_scrollback_navigation() {
        let shift = gtk::gdk::ModifierType::SHIFT_MASK;
        let none = gtk::gdk::ModifierType::empty();
        let ctrl_shift = gtk::gdk::ModifierType::CONTROL_MASK | gtk::gdk::ModifierType::SHIFT_MASK;

        assert_eq!(
            scrollback_navigation_for_key(gtk::gdk::Key::Page_Up, shift),
            Some(ScrollbackNavigation::PageUp)
        );
        assert_eq!(
            scrollback_navigation_for_key(gtk::gdk::Key::KP_Page_Down, shift),
            Some(ScrollbackNavigation::PageDown)
        );
        assert_eq!(
            scrollback_navigation_for_key(gtk::gdk::Key::Home, shift),
            Some(ScrollbackNavigation::Top)
        );
        assert_eq!(
            scrollback_navigation_for_key(gtk::gdk::Key::End, shift),
            Some(ScrollbackNavigation::Bottom)
        );
        assert_eq!(
            scrollback_navigation_for_key(gtk::gdk::Key::KP_Page_Up, shift),
            Some(ScrollbackNavigation::PageUp)
        );
        assert_eq!(
            scrollback_navigation_for_key(gtk::gdk::Key::KP_Home, shift),
            Some(ScrollbackNavigation::Top)
        );
        assert_eq!(
            scrollback_navigation_for_key(gtk::gdk::Key::KP_End, shift),
            Some(ScrollbackNavigation::Bottom)
        );
        // Without Shift (or with extra modifiers) the keys still belong to the
        // application / existing accelerators.
        assert_eq!(
            scrollback_navigation_for_key(gtk::gdk::Key::Page_Up, none),
            None
        );
        assert_eq!(
            scrollback_navigation_for_key(gtk::gdk::Key::Page_Up, ctrl_shift),
            None
        );
        assert_eq!(scrollback_navigation_for_key(gtk::gdk::Key::a, shift), None);
    }

    #[test]
    fn navigation_fallback_policy_routes_from_app_chrome_but_not_text_or_popovers() {
        assert!(terminal_navigation_fallback_allowed(
            TerminalNavigationFallbackFocus::AppChrome
        ));
        assert!(terminal_navigation_fallback_allowed(
            TerminalNavigationFallbackFocus::TerminalSurface
        ));
        assert!(!terminal_navigation_fallback_allowed(
            TerminalNavigationFallbackFocus::TextInput
        ));
        assert!(!terminal_navigation_fallback_allowed(
            TerminalNavigationFallbackFocus::Popover
        ));
    }
}
