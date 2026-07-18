//! tmux key-name → VT input-byte translation (PROTOCOL.md §7.9).
//!
//! The daemon and TUI already speak tmux's key vocabulary (`Enter`, `Escape`, `C-c`,
//! `M-BSpace`, `C-Up`, …); on Unix tmux turns those names into terminal input sequences.
//! This module is that key table for the Windows host: conventional VT/xterm sequences,
//! matching what tmux itself writes to a pane.

/// Translate a tmux key name into the bytes to write to the child's input. `None` for
/// names outside the vocabulary (the caller answers `err`).
pub fn key_to_bytes(key: &str) -> Option<Vec<u8>> {
    let (mut ctrl, mut alt) = (false, false);
    let mut rest = key;
    loop {
        if let Some(r) = rest.strip_prefix("C-") {
            if r.is_empty() {
                return None;
            }
            ctrl = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("M-") {
            if r.is_empty() {
                return None;
            }
            alt = true;
            rest = r;
        } else {
            break;
        }
    }
    if rest.is_empty() {
        return None;
    }

    // A single character stands for itself (Ctrl folds it to a control byte).
    let mut chars = rest.chars();
    let c = chars.next().expect("non-empty");
    if chars.next().is_none() {
        let mut out = Vec::new();
        if alt {
            out.push(0x1b);
        }
        if ctrl {
            out.push(ctrl_byte(c)?);
        } else {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
        return Some(out);
    }

    named_key(rest, ctrl, alt)
}

/// The control byte for `Ctrl+<c>` (`C-a` → 0x01, `C-Space`/`C-@` → 0x00, `C-[` → ESC …).
fn ctrl_byte(c: char) -> Option<u8> {
    match c.to_ascii_uppercase() {
        c @ 'A'..='Z' => Some(c as u8 - b'A' + 1),
        '@' => Some(0x00),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        '?' => Some(0x7f),
        _ => None,
    }
}

/// How a named key is encoded, both plain and with xterm CSI modifiers.
enum Named {
    /// Plain bytes; Alt prefixes ESC, Ctrl has no distinct encoding (base is sent).
    Plain(&'static [u8]),
    /// `ESC [ <final>` plain, `ESC [ 1 ; <mod> <final>` modified (arrows).
    CsiLetter(u8),
    /// `ESC [ <num> ~` plain, `ESC [ <num> ; <mod> ~` modified (DC/IC/Page*/F5+).
    Tilde(u8),
    /// tmux-default `ESC [ <num> ~` plain, xterm `ESC [ 1 ; <mod> <final>` modified.
    HomeEnd(u8, u8),
    /// `ESC O <final>` plain, `ESC [ 1 ; <mod> <final>` modified (F1–F4).
    Ss3(u8),
}

fn named_key(name: &str, ctrl: bool, alt: bool) -> Option<Vec<u8>> {
    use Named::*;
    if name == "Space" {
        let byte = if ctrl { 0x00 } else { b' ' };
        return Some(if alt { vec![0x1b, byte] } else { vec![byte] });
    }
    let kind = match name {
        "Enter" => Plain(b"\r"),
        "Escape" => Plain(b"\x1b"),
        "Tab" => Plain(b"\t"),
        "BTab" => Plain(b"\x1b[Z"),
        "BSpace" => Plain(b"\x7f"),
        "Up" => CsiLetter(b'A'),
        "Down" => CsiLetter(b'B'),
        "Right" => CsiLetter(b'C'),
        "Left" => CsiLetter(b'D'),
        "Home" => HomeEnd(1, b'H'),
        "End" => HomeEnd(4, b'F'),
        "IC" => Tilde(2),
        "DC" => Tilde(3),
        "PageUp" | "PgUp" => Tilde(5),
        "PageDown" | "PgDn" => Tilde(6),
        "F1" => Ss3(b'P'),
        "F2" => Ss3(b'Q'),
        "F3" => Ss3(b'R'),
        "F4" => Ss3(b'S'),
        "F5" => Tilde(15),
        "F6" => Tilde(17),
        "F7" => Tilde(18),
        "F8" => Tilde(19),
        "F9" => Tilde(20),
        "F10" => Tilde(21),
        "F11" => Tilde(23),
        "F12" => Tilde(24),
        _ => return None,
    };
    // xterm modifier parameter: 1 + 2·Alt + 4·Ctrl.
    let modifier = 1 + if alt { 2 } else { 0 } + if ctrl { 4 } else { 0 };
    let modified = ctrl || alt;
    Some(match kind {
        Plain(bytes) => {
            let mut out = Vec::new();
            if alt {
                out.push(0x1b);
            }
            out.extend_from_slice(bytes);
            out
        }
        CsiLetter(l) if !modified => vec![0x1b, b'[', l],
        CsiLetter(l) => format!("\x1b[1;{modifier}{}", l as char).into_bytes(),
        Tilde(n) if !modified => format!("\x1b[{n}~").into_bytes(),
        Tilde(n) => format!("\x1b[{n};{modifier}~").into_bytes(),
        HomeEnd(n, _) if !modified => format!("\x1b[{n}~").into_bytes(),
        HomeEnd(_, l) => format!("\x1b[1;{modifier}{}", l as char).into_bytes(),
        Ss3(l) if !modified => vec![0x1b, b'O', l],
        Ss3(l) => format!("\x1b[1;{modifier}{}", l as char).into_bytes(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn bytes(name: &str) -> Vec<u8> {
        key_to_bytes(name).unwrap_or_else(|| panic!("key {name:?} should translate"))
    }

    #[test]
    fn named_keys_translate() {
        assert_eq!(bytes("Enter"), b"\r");
        assert_eq!(bytes("Escape"), b"\x1b");
        assert_eq!(bytes("Tab"), b"\t");
        assert_eq!(bytes("BTab"), b"\x1b[Z");
        assert_eq!(bytes("BSpace"), b"\x7f");
        assert_eq!(bytes("Space"), b" ");
        assert_eq!(bytes("DC"), b"\x1b[3~");
        assert_eq!(bytes("Home"), b"\x1b[1~");
        assert_eq!(bytes("End"), b"\x1b[4~");
        assert_eq!(bytes("PageUp"), b"\x1b[5~");
        assert_eq!(bytes("PageDown"), b"\x1b[6~");
        assert_eq!(bytes("Up"), b"\x1b[A");
        assert_eq!(bytes("Down"), b"\x1b[B");
        assert_eq!(bytes("Right"), b"\x1b[C");
        assert_eq!(bytes("Left"), b"\x1b[D");
        assert_eq!(bytes("F1"), b"\x1bOP");
        assert_eq!(bytes("F5"), b"\x1b[15~");
        assert_eq!(bytes("F12"), b"\x1b[24~");
    }

    #[test]
    fn single_characters_stand_for_themselves() {
        assert_eq!(bytes("a"), b"a");
        assert_eq!(bytes("Z"), b"Z");
        assert_eq!(bytes("/"), b"/");
        // Multi-byte UTF-8 chars pass through too.
        assert_eq!(bytes("é"), "é".as_bytes());
    }

    #[test]
    fn ctrl_characters_become_control_bytes() {
        assert_eq!(bytes("C-c"), vec![0x03]);
        assert_eq!(bytes("C-a"), vec![0x01]);
        assert_eq!(bytes("C-z"), vec![0x1a]);
        // tmux accepts either case for the letter.
        assert_eq!(bytes("C-C"), vec![0x03]);
        assert_eq!(bytes("C-Space"), vec![0x00]);
        assert_eq!(bytes("C-["), vec![0x1b]);
        assert_eq!(bytes("C-_"), vec![0x1f]);
    }

    #[test]
    fn meta_prefixes_escape() {
        assert_eq!(bytes("M-f"), b"\x1bf");
        assert_eq!(bytes("M-b"), b"\x1bb");
        assert_eq!(bytes("M-BSpace"), b"\x1b\x7f");
        assert_eq!(bytes("M-Enter"), b"\x1b\r");
    }

    #[test]
    fn modified_named_keys_use_csi_modifiers() {
        // xterm modifier encoding: 5 = Ctrl, 3 = Alt.
        assert_eq!(bytes("C-Up"), b"\x1b[1;5A");
        assert_eq!(bytes("C-Right"), b"\x1b[1;5C");
        assert_eq!(bytes("M-Up"), b"\x1b[1;3A");
        assert_eq!(bytes("M-Left"), b"\x1b[1;3D");
        assert_eq!(bytes("C-Home"), b"\x1b[1;5H");
        assert_eq!(bytes("C-PageUp"), b"\x1b[5;5~");
        assert_eq!(bytes("C-DC"), b"\x1b[3;5~");
    }

    #[test]
    fn unknown_keys_are_none() {
        assert_eq!(key_to_bytes("Fnord"), None);
        assert_eq!(key_to_bytes("C-Fnord"), None);
        assert_eq!(key_to_bytes(""), None);
    }
}
