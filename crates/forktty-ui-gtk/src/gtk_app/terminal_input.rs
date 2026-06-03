use super::*;

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

pub(super) fn encode_gtk_key(
    key: gtk::gdk::Key,
    modifiers: gtk::gdk::ModifierType,
    text: Option<&str>,
) -> Option<Vec<u8>> {
    if is_forktty_accelerator(key, modifiers) {
        return None;
    }
    let ctrl = modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
    let spec = match key {
        gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter => GhosttyKeySpec {
            key: GhosttyKey::Enter,
            ctrl,
        },
        gtk::gdk::Key::BackSpace => return Some(vec![0x7f]),
        gtk::gdk::Key::Tab => return Some(b"\t".to_vec()),
        gtk::gdk::Key::Escape => return Some(b"\x1b".to_vec()),
        _ => {
            if let Some(ch) = key.to_unicode() {
                GhosttyKeySpec {
                    key: GhosttyKey::Char(ch),
                    ctrl,
                }
            } else if !ctrl {
                return text.map(|text| text.as_bytes().to_vec());
            } else {
                return None;
            }
        }
    };
    encode_key(spec)
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
    let shift = modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK);
    ctrl && shift
        && matches!(
            key,
            gtk::gdk::Key::H
                | gtk::gdk::Key::h
                | gtk::gdk::Key::E
                | gtk::gdk::Key::e
                | gtk::gdk::Key::T
                | gtk::gdk::Key::t
                | gtk::gdk::Key::W
                | gtk::gdk::Key::w
                | gtk::gdk::Key::R
                | gtk::gdk::Key::r
        )
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
}
