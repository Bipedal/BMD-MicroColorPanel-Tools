//! Keyboard combinations by name: `"ctrl+shift+c"`, `"alt+left"` -> key codes.
//!
//! The codes are Windows virtual-key codes on every system; `sys` turns them into key presses.

/// A key combination: zero or more modifiers plus exactly one key.
/// All values are Windows virtual-key codes.
#[derive(Debug, Clone, PartialEq)]
pub struct Combo {
    /// Modifier virtual-key codes, in the order they were written.
    pub mods: Vec<u16>,
    /// The non-modifier key.
    pub key: u16,
}

const VK_CONTROL: u16 = 0x11;
const VK_SHIFT: u16 = 0x10;
const VK_MENU: u16 = 0x12;
const VK_LWIN: u16 = 0x5B;

/// Parses a combo such as `"ctrl+shift+c"`, `"f5"` or `"ctrl+alt+delete"`.
///
/// Matching is case-insensitive and whitespace around each `+` is ignored.
/// A modifier on its own ("alt", "ctrl+shift") is the key itself (hold = true makes a held Alt).
/// Otherwise exactly one non-modifier key; duplicate
/// modifiers, duplicate keys and unknown names are errors.
pub fn parse_combo(s: &str) -> Result<Combo, String> {
    let whole = s.trim();
    if whole.is_empty() {
        return Err("empty key combo".to_string());
    }

    let mut mods: Vec<u16> = Vec::new();
    let mut key: Option<(u16, String)> = None;

    for part in whole.split('+') {
        let raw = part.trim();
        if raw.is_empty() {
            return Err(format!("empty key name in combo '{whole}'"));
        }
        let tok = raw.to_ascii_lowercase();

        if let Some(vk) = modifier_vk(&tok) {
            if mods.contains(&vk) {
                return Err(format!("duplicate modifier '{raw}' in combo '{whole}'"));
            }
            mods.push(vk);
        } else if let Some(vk) = key_vk(&tok) {
            if let Some((_, prev)) = &key {
                if *prev == tok {
                    return Err(format!("duplicate key '{raw}' in combo '{whole}'"));
                }
                return Err(format!(
                    "combo '{whole}' has more than one key ('{prev}' and '{raw}')"
                ));
            }
            key = Some((vk, tok));
        } else {
            return Err(format!("unknown key name '{raw}' in combo '{whole}'"));
        }
    }

    match key {
        Some((vk, _)) => Ok(Combo { mods, key: vk }),
        None => {
            let last = mods.pop().expect("a non-empty combo has at least one token");
            Ok(Combo { mods, key: last })
        }
    }
}

/// Virtual-key code for a (lowercase) modifier name, if it is one.
fn modifier_vk(name: &str) -> Option<u16> {
    match name {
        "ctrl" | "control" => Some(VK_CONTROL),
        "shift" => Some(VK_SHIFT),
        "alt" => Some(VK_MENU),
        "win" | "super" | "meta" => Some(VK_LWIN),
        _ => None,
    }
}

/// Virtual-key code for a (lowercase) non-modifier key name.
fn key_vk(name: &str) -> Option<u16> {
    let bytes = name.as_bytes();
    if bytes.len() == 1 {
        let c = bytes[0];
        if c.is_ascii_lowercase() {
            return Some(0x41 + u16::from(c - b'a'));
        }
        if c.is_ascii_digit() {
            return Some(0x30 + u16::from(c - b'0'));
        }
    }

    if let Some(num) = name.strip_prefix('f')
        && let Ok(n) = num.parse::<u8>()
        && (1..=24).contains(&n)
    {
        return Some(0x70 + u16::from(n - 1));
    }

    if let Some(digit) = name.strip_prefix("numpad")
        && digit.len() == 1
        && let Some(d) = digit.bytes().next().filter(u8::is_ascii_digit)
    {
        return Some(0x60 + u16::from(d - b'0'));
    }

    Some(match name {
        "space" => 0x20,
        "enter" | "return" => 0x0D,
        "tab" => 0x09,
        "esc" | "escape" => 0x1B,
        "backspace" => 0x08,
        "delete" | "del" => 0x2E,
        "insert" | "ins" => 0x2D,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" | "pgup" => 0x21,
        "pagedown" | "pgdn" => 0x22,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "comma" => 0xBC,
        "period" | "dot" => 0xBE,
        "minus" => 0xBD,
        "plus" | "equals" => 0xBB,
        "slash" => 0xBF,
        "backslash" => 0xDC,
        "semicolon" => 0xBA,
        "quote" => 0xDE,
        "bracketleft" | "lbracket" => 0xDB,
        "bracketright" | "rbracket" => 0xDD,
        "backquote" | "grave" => 0xC0,
        "printscreen" => 0x2C,
        "pause" => 0x13,
        "capslock" => 0x14,
        "menu" | "apps" => 0x5D,
        "volumeup" => 0xAF,
        "volumedown" => 0xAE,
        "mute" => 0xAD,
        "mediaplay" => 0xB3,
        "medianext" => 0xB0,
        "mediaprev" => 0xB1,
        _ => return None,
    })
}

/// Name of a non-modifier virtual key (the reverse of the names above), for recording.
pub fn vk_name(vk: u16) -> Option<String> {
    const NAMED: [&str; 39] = ["space", "enter", "tab", "esc", "backspace", "delete", "insert", "home", "end",
        "pageup", "pagedown", "left", "up", "right", "down", "comma", "period", "minus", "plus", "slash",
        "backslash", "semicolon", "quote", "bracketleft", "bracketright", "backquote", "printscreen", "pause",
        "capslock", "menu", "volumeup", "volumedown", "mute", "mediaplay", "medianext", "mediaprev",
        "numpad0", "f1", "a"];
    let mut cands: Vec<String> = NAMED.iter().map(|s| s.to_string()).collect();
    cands.extend((b'a'..=b'z').map(|c| (c as char).to_string()));
    cands.extend((b'0'..=b'9').map(|c| (c as char).to_string()));
    cands.extend((1..=24).map(|n| format!("f{n}")));
    cands.extend((0..=9).map(|n| format!("numpad{n}")));
    cands.into_iter().find(|n| key_vk(n) == Some(vk))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> Combo {
        parse_combo(s).unwrap_or_else(|e| panic!("'{s}' should parse, got: {e}"))
    }

    fn err(s: &str) -> String {
        match parse_combo(s) {
            Ok(c) => panic!("'{s}' should fail, got {c:?}"),
            Err(e) => e,
        }
    }

    #[test]
    fn modifiers_and_letter() {
        assert_eq!(
            ok("ctrl+shift+c"),
            Combo { mods: vec![0x11, 0x10], key: 0x43 }
        );
        assert_eq!(ok("alt+left"), Combo { mods: vec![0x12], key: 0x25 });
        assert_eq!(ok("win+d"), Combo { mods: vec![0x5B], key: 0x44 });
        assert_eq!(
            ok("ctrl+alt+delete"),
            Combo { mods: vec![0x11, 0x12], key: 0x2E }
        );
    }

    #[test]
    fn bare_keys_have_no_modifiers() {
        assert_eq!(ok("f5"), Combo { mods: vec![], key: 0x74 });
        assert_eq!(ok("space"), Combo { mods: vec![], key: 0x20 });
    }

    #[test]
    fn case_and_whitespace_are_ignored() {
        assert_eq!(ok("  Ctrl + Shift +  C "), ok("ctrl+shift+c"));
        assert_eq!(ok("WIN+D"), ok("win+d"));
        assert_eq!(ok("Space"), ok("space"));
    }

    #[test]
    fn modifier_order_is_kept() {
        assert_eq!(ok("shift+ctrl+a").mods, vec![0x10, 0x11]);
        assert_eq!(ok("shift+ctrl+a").key, 0x41);
    }

    #[test]
    fn modifier_aliases() {
        assert_eq!(ok("control+x").mods, vec![0x11]);
        assert_eq!(ok("super+x").mods, vec![0x5B]);
        assert_eq!(ok("meta+x").mods, vec![0x5B]);
    }

    #[test]
    fn key_names_and_codes() {
        assert_eq!(ok("a").key, 0x41);
        assert_eq!(ok("z").key, 0x5A);
        assert_eq!(ok("0").key, 0x30);
        assert_eq!(ok("9").key, 0x39);
        assert_eq!(ok("f1").key, 0x70);
        assert_eq!(ok("f12").key, 0x7B);
        assert_eq!(ok("f24").key, 0x87);
        assert_eq!(ok("numpad0").key, 0x60);
        assert_eq!(ok("numpad9").key, 0x69);
        assert_eq!(ok("return").key, 0x0D);
        assert_eq!(ok("enter").key, 0x0D);
        assert_eq!(ok("escape").key, 0x1B);
        assert_eq!(ok("esc").key, 0x1B);
        assert_eq!(ok("del").key, 0x2E);
        assert_eq!(ok("ins").key, 0x2D);
        assert_eq!(ok("pgup").key, 0x21);
        assert_eq!(ok("pgdn").key, 0x22);
        assert_eq!(ok("dot").key, 0xBE);
        assert_eq!(ok("equals").key, 0xBB);
        assert_eq!(ok("lbracket").key, 0xDB);
        assert_eq!(ok("rbracket").key, 0xDD);
        assert_eq!(ok("grave").key, 0xC0);
        assert_eq!(ok("apps").key, 0x5D);
        assert_eq!(ok("volumeup").key, 0xAF);
        assert_eq!(ok("volumedown").key, 0xAE);
        assert_eq!(ok("mute").key, 0xAD);
        assert_eq!(ok("mediaplay").key, 0xB3);
        assert_eq!(ok("medianext").key, 0xB0);
        assert_eq!(ok("mediaprev").key, 0xB1);
        assert_eq!(ok("printscreen").key, 0x2C);
        assert_eq!(ok("pause").key, 0x13);
        assert_eq!(ok("capslock").key, 0x14);
    }

    #[test]
    fn vk_names_round_trip() {
        for n in ["w", "7", "f12", "numpad3", "bracketleft", "space", "esc"] {
            assert_eq!(vk_name(key_vk(n).unwrap()).as_deref(), Some(n));
        }
        assert_eq!(vk_name(0x12), None, "modifiers are not keys here");
    }

    #[test]
    fn empty_and_modifier_only_are_errors() {
        assert!(err("").contains("empty"));
        assert!(err("   ").contains("empty"));
        assert_eq!(ok("alt"), Combo { mods: vec![], key: 0x12 });
        assert_eq!(ok("ctrl+shift"), Combo { mods: vec![0x11], key: 0x10 });
    }

    #[test]
    fn unknown_key_names_the_token() {
        assert!(err("ctrl+foo").contains("foo"));
        assert!(err("f25").contains("f25"));
        assert!(err("f0").contains("f0"));
        assert!(err("numpad10").contains("numpad10"));
        assert!(err("ctrl+Bogus").contains("Bogus"));
    }

    #[test]
    fn empty_token_is_an_error() {
        assert!(err("ctrl++c").contains("empty"));
        assert!(err("+c").contains("empty"));
        assert!(err("c+").contains("empty"));
    }

    #[test]
    fn duplicates_are_errors() {
        assert!(err("ctrl+ctrl+c").contains("ctrl"));
        assert!(err("ctrl+control+c").contains("control"));
        assert!(err("a+a").contains("duplicate key"));
        assert!(err("a+b").contains("more than one key"));
        assert!(err("ctrl+a+b").contains("more than one key"));
    }

}
