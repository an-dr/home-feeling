//! Hotkey strings such as `alt+shift+left`, parsed without any platform.

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub win: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// `a` to `z`, upper case.
    Letter(char),
    Digit(char),
    Left,
    Right,
    Up,
    Down,
    Space,
    Enter,
    Tab,
    Function(u8),
    /// Caps Lock, taken over: it fires the hotkey and never toggles capitals.
    CapsLock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub modifiers: Modifiers,
    pub key: Key,
}

impl Hotkey {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut modifiers = Modifiers::default();
        let mut key = None;
        for part in text.split('+').map(|p| p.trim().to_ascii_lowercase()) {
            match part.as_str() {
                "alt" => modifiers.alt = true,
                "ctrl" | "control" => modifiers.ctrl = true,
                "shift" => modifiers.shift = true,
                "win" | "super" | "meta" => modifiers.win = true,
                other => {
                    if key.is_some() {
                        return Err(format!("'{text}' names more than one key"));
                    }
                    key = Some(
                        parse_key(other)
                            .ok_or_else(|| format!("unknown key '{other}' in '{text}'"))?,
                    );
                }
            }
        }
        let key = key.ok_or_else(|| format!("'{text}' names no key"))?;
        Ok(Self { modifiers, key })
    }
}

impl std::fmt::Display for Hotkey {
    /// The text form `parse` reads back: modifiers in a fixed order, then the key.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let m = self.modifiers;
        for (on, name) in [
            (m.win, "win"),
            (m.ctrl, "ctrl"),
            (m.alt, "alt"),
            (m.shift, "shift"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        match self.key {
            Key::Letter(c) | Key::Digit(c) => write!(f, "{}", c.to_ascii_lowercase()),
            Key::Left => write!(f, "left"),
            Key::Right => write!(f, "right"),
            Key::Up => write!(f, "up"),
            Key::Down => write!(f, "down"),
            Key::Space => write!(f, "space"),
            Key::Enter => write!(f, "enter"),
            Key::Tab => write!(f, "tab"),
            Key::Function(n) => write!(f, "f{n}"),
            Key::CapsLock => write!(f, "capslock"),
        }
    }
}

fn parse_key(name: &str) -> Option<Key> {
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => {
            return Some(Key::Letter(c.to_ascii_uppercase()))
        }
        (Some(c), None) if c.is_ascii_digit() => return Some(Key::Digit(c)),
        _ => {}
    }
    Some(match name {
        "left" => Key::Left,
        "right" => Key::Right,
        "up" => Key::Up,
        "down" => Key::Down,
        "space" => Key::Space,
        "enter" | "return" => Key::Enter,
        "tab" => Key::Tab,
        "capslock" | "caps" => Key::CapsLock,
        _ => {
            let number: u8 = name.strip_prefix('f')?.parse().ok()?;
            if !(1..=12).contains(&number) {
                return None;
            }
            Key::Function(number)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{Hotkey, Key};

    #[test]
    fn parses_modifiers_and_keys() {
        let hotkey = Hotkey::parse("Alt+Shift+Left").unwrap();
        assert!(hotkey.modifiers.alt && hotkey.modifiers.shift && !hotkey.modifiers.ctrl);
        assert_eq!(hotkey.key, Key::Left);
        assert_eq!(Hotkey::parse("win+f").unwrap().key, Key::Letter('F'));
        assert_eq!(Hotkey::parse("ctrl+f5").unwrap().key, Key::Function(5));
    }

    #[test]
    fn text_form_reads_back_as_the_same_hotkey() {
        for text in [
            "alt+shift+left",
            "ctrl+alt+f5",
            "win+a",
            "alt+shift+space",
            "ctrl+1",
            "capslock",
            "shift+capslock",
        ] {
            let hotkey = Hotkey::parse(text).unwrap();
            assert_eq!(hotkey.to_string(), text);
            assert_eq!(Hotkey::parse(&hotkey.to_string()).unwrap(), hotkey);
        }
    }

    #[test]
    fn rejects_unknown_or_missing_keys() {
        assert!(Hotkey::parse("alt+banana").is_err());
        assert!(Hotkey::parse("alt+shift").is_err());
        assert!(Hotkey::parse("a+b").is_err());
        assert!(Hotkey::parse("f13").is_err());
    }
}
