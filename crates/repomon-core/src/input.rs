//! Shared terminal-key translation for every interactive client.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Escape,
    Enter,
    Backspace,
    Tab,
    BackTab,
    Up,
    Down,
    Left,
    Right,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub control: bool,
    pub alt: bool,
}

/// Return the tmux key specification and whether it should be sent as literal text.
pub fn translate_key(key: Key, modifiers: Modifiers) -> (String, bool) {
    if let Key::Char(character) = key {
        if modifiers.control {
            return (format!("C-{}", character.to_ascii_lowercase()), false);
        }
        if modifiers.alt {
            return (format!("M-{character}"), false);
        }
        return (character.to_string(), true);
    }

    let base = match key {
        Key::Escape => "Escape",
        Key::Enter => "Enter",
        Key::Backspace => "BSpace",
        Key::Tab => "Tab",
        Key::BackTab => "BTab",
        Key::Up => "Up",
        Key::Down => "Down",
        Key::Left => "Left",
        Key::Right => "Right",
        Key::Delete => "DC",
        Key::Home => "Home",
        Key::End => "End",
        Key::PageUp => "PageUp",
        Key::PageDown => "PageDown",
        Key::Char(_) => unreachable!(),
    };
    let prefix = if modifiers.control {
        "C-"
    } else if modifiers.alt {
        "M-"
    } else {
        ""
    };
    (format!("{prefix}{base}"), false)
}

#[cfg(test)]
mod tests {
    use super::{Key, Modifiers, translate_key};

    #[test]
    fn printable_and_modified_keys_match_tmux_syntax() {
        assert_eq!(
            translate_key(Key::Char('o'), Modifiers::default()),
            ("o".into(), true)
        );
        assert_eq!(
            translate_key(
                Key::Char('O'),
                Modifiers {
                    control: true,
                    alt: false
                }
            ),
            ("C-o".into(), false)
        );
        assert_eq!(
            translate_key(
                Key::Left,
                Modifiers {
                    control: false,
                    alt: true
                }
            ),
            ("M-Left".into(), false)
        );
        assert_eq!(
            translate_key(
                Key::Backspace,
                Modifiers {
                    control: true,
                    alt: false
                }
            ),
            ("C-BSpace".into(), false)
        );
    }
}
