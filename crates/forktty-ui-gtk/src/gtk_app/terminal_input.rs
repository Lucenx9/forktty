use super::*;
use forktty_terminal::ghostty::core::{TerminalKey, TerminalKeyInput, TerminalKeyModifiers};

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

#[cfg(test)]
fn encode_gtk_key(
    key: gtk::gdk::Key,
    modifiers: gtk::gdk::ModifierType,
    text: Option<&str>,
) -> Option<Vec<u8>> {
    match translate_gtk_key(key, modifiers, text)? {
        TerminalInput::Bytes(bytes) => Some(bytes),
        TerminalInput::Key(_) => None,
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
    let terminal_modifiers = terminal_key_modifiers(modifiers);
    let spec = match key {
        gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter => GhosttyKeySpec {
            key: GhosttyKey::Enter,
            ctrl,
        },
        gtk::gdk::Key::BackSpace => return Some(TerminalInput::Bytes(vec![0x7f])),
        gtk::gdk::Key::Tab => return Some(TerminalInput::Bytes(b"\t".to_vec())),
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
            if let Some(ch) = key.to_unicode() {
                GhosttyKeySpec {
                    key: GhosttyKey::Char(ch),
                    ctrl,
                }
            } else if !ctrl {
                return text.map(|text| TerminalInput::Bytes(text.as_bytes().to_vec()));
            } else {
                return None;
            }
        }
    };
    encode_key(spec).map(TerminalInput::Bytes)
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

fn terminal_key_modifiers(modifiers: gtk::gdk::ModifierType) -> TerminalKeyModifiers {
    TerminalKeyModifiers {
        shift: modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK),
        alt: modifiers.contains(gtk::gdk::ModifierType::ALT_MASK),
        ctrl: modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK),
    }
}

fn control_code(ch: char) -> Option<u8> {
    let lower = ch.to_ascii_lowercase();
    if lower.is_ascii_lowercase() {
        Some((lower as u8) - b'a' + 1)
    } else {
        None
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
                | gtk::gdk::Key::Page_Down
                | gtk::gdk::Key::Home
                | gtk::gdk::Key::End
        );
    let pane_focus_action = ctrl
        && alt
        && matches!(
            key,
            gtk::gdk::Key::Left
                | gtk::gdk::Key::KP_Left
                | gtk::gdk::Key::Right
                | gtk::gdk::Key::KP_Right
        );
    ctrl_shift_app_action || ctrl_app_action || tab_navigation_action || pane_focus_action
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
    fn key_translation_leaves_ctrl_alt_arrows_for_pane_shortcuts() {
        let pane_shortcut = gtk::gdk::ModifierType::CONTROL_MASK | gtk::gdk::ModifierType::ALT_MASK;

        assert!(encode_gtk_key(gtk::gdk::Key::Left, pane_shortcut, None).is_none());
        assert!(encode_gtk_key(gtk::gdk::Key::Right, pane_shortcut, None).is_none());
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
    }
}
