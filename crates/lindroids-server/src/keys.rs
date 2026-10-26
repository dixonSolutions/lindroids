//! Keyboard translation. Clients get the US evdev keymap in `data/keymap-us.xkb`, so every key we
//! send is a Linux evdev code from that layout.

/// The keymap handed to every client (text, XKB v1 format).
pub const KEYMAP_US: &str = include_str!("../data/keymap-us.xkb");

pub const KEY_ESC: u32 = 1;
pub const KEY_BACKSPACE: u32 = 14;
pub const KEY_TAB: u32 = 15;
pub const KEY_ENTER: u32 = 28;
pub const KEY_LEFTCTRL: u32 = 29;
pub const KEY_LEFTSHIFT: u32 = 42;
pub const KEY_RIGHTSHIFT: u32 = 54;
pub const KEY_LEFTALT: u32 = 56;
pub const KEY_SPACE: u32 = 57;
pub const KEY_CAPSLOCK: u32 = 58;
pub const KEY_RIGHTCTRL: u32 = 97;
pub const KEY_RIGHTALT: u32 = 100;
pub const KEY_LEFTMETA: u32 = 125;
pub const KEY_RIGHTMETA: u32 = 126;

/// Modifier masks for the embedded keymap (real modifier order: Shift, Lock, Control, Mod1..Mod5).
pub const MOD_SHIFT: u32 = 1 << 0;
pub const MOD_LOCK: u32 = 1 << 1;
pub const MOD_CTRL: u32 = 1 << 2;
pub const MOD_ALT: u32 = 1 << 3;
pub const MOD_SUPER: u32 = 1 << 6;
pub const MOD_ALTGR: u32 = 1 << 7;

/// Android `KeyEvent.KEYCODE_*` to Linux evdev. `None` for keys with no Linux equivalent.
pub fn android_to_evdev(code: i32) -> Option<u32> {
    Some(match code {
        // letters: KEYCODE_A (29) .. KEYCODE_Z (54)
        29 => 30, 30 => 48, 31 => 46, 32 => 32, 33 => 18, 34 => 33, 35 => 34, 36 => 35, 37 => 23,
        38 => 36, 39 => 37, 40 => 38, 41 => 50, 42 => 49, 43 => 24, 44 => 25, 45 => 16, 46 => 19,
        47 => 31, 48 => 20, 49 => 22, 50 => 47, 51 => 17, 52 => 45, 53 => 21, 54 => 44,
        // digits: KEYCODE_0 (7) .. KEYCODE_9 (16)
        7 => 11, 8 => 2, 9 => 3, 10 => 4, 11 => 5, 12 => 6, 13 => 7, 14 => 8, 15 => 9, 16 => 10,
        62 => KEY_SPACE,
        66 => KEY_ENTER,
        67 => KEY_BACKSPACE,
        112 => 111, // FORWARD_DEL -> DELETE
        61 => KEY_TAB,
        111 => KEY_ESC,
        4 => KEY_ESC, // BACK from a hardware keyboard
        59 => KEY_LEFTSHIFT,
        60 => KEY_RIGHTSHIFT,
        113 => KEY_LEFTCTRL,
        114 => KEY_RIGHTCTRL,
        57 => KEY_LEFTALT,
        58 => KEY_RIGHTALT,
        117 => KEY_LEFTMETA,
        118 => KEY_RIGHTMETA,
        19 => 103, // DPAD_UP
        20 => 108, // DPAD_DOWN
        21 => 105, // DPAD_LEFT
        22 => 106, // DPAD_RIGHT
        23 => KEY_ENTER, // DPAD_CENTER
        122 => 102, // MOVE_HOME
        123 => 107, // MOVE_END
        92 => 104,  // PAGE_UP
        93 => 109,  // PAGE_DOWN
        124 => 110, // INSERT
        131 => 59, 132 => 60, 133 => 61, 134 => 62, 135 => 63, 136 => 64, 137 => 65, 138 => 66,
        139 => 67, 140 => 68, 141 => 87, 142 => 88, // F1..F12
        55 => 51,  // COMMA
        56 => 52,  // PERIOD
        69 => 12,  // MINUS
        70 => 13,  // EQUALS
        71 => 26,  // LEFT_BRACKET
        72 => 27,  // RIGHT_BRACKET
        73 => 43,  // BACKSLASH
        74 => 39,  // SEMICOLON
        75 => 40,  // APOSTROPHE
        76 => 53,  // SLASH
        68 => 41,  // GRAVE
        115 => KEY_CAPSLOCK,
        143 => 69,  // NUM_LOCK
        116 => 70,  // SCROLL_LOCK
        120 => 99,  // SYSRQ
        121 => 119, // BREAK -> PAUSE
        82 => 127,  // MENU -> COMPOSE
        144 => 82, 145 => 79, 146 => 80, 147 => 81, 148 => 75, 149 => 76, 150 => 77, 151 => 71,
        152 => 72, 153 => 73, // NUMPAD_0..9
        154 => 98, 155 => 55, 156 => 74, 157 => 78, 158 => 83, 159 => 121, 160 => 96, 161 => 117,
        _ => return None,
    })
}

/// Which modifier a key is, as a mask, if any.
pub fn modifier_of(evdev: u32) -> u32 {
    match evdev {
        KEY_LEFTSHIFT | KEY_RIGHTSHIFT => MOD_SHIFT,
        KEY_LEFTCTRL | KEY_RIGHTCTRL => MOD_CTRL,
        KEY_LEFTALT => MOD_ALT,
        KEY_RIGHTALT => MOD_ALTGR,
        KEY_LEFTMETA | KEY_RIGHTMETA => MOD_SUPER,
        _ => 0,
    }
}

/// A character the US layout can type, as (evdev key, needs shift).
pub fn char_to_key(c: char) -> Option<(u32, bool)> {
    const LETTERS: [u32; 26] = [
        30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44,
    ];
    Some(match c {
        'a'..='z' => (LETTERS[(c as u8 - b'a') as usize], false),
        'A'..='Z' => (LETTERS[(c as u8 - b'A') as usize], true),
        '1'..='9' => ((c as u8 - b'1') as u32 + 2, false),
        '0' => (11, false),
        '!' => (2, true), '@' => (3, true), '#' => (4, true), '$' => (5, true), '%' => (6, true),
        '^' => (7, true), '&' => (8, true), '*' => (9, true), '(' => (10, true), ')' => (11, true),
        '-' => (12, false), '_' => (12, true), '=' => (13, false), '+' => (13, true),
        '[' => (26, false), '{' => (26, true), ']' => (27, false), '}' => (27, true),
        '\\' => (43, false), '|' => (43, true), ';' => (39, false), ':' => (39, true),
        '\'' => (40, false), '"' => (40, true), '`' => (41, false), '~' => (41, true),
        ',' => (51, false), '<' => (51, true), '.' => (52, false), '>' => (52, true),
        '/' => (53, false), '?' => (53, true),
        ' ' => (KEY_SPACE, false),
        '\n' | '\r' => (KEY_ENTER, false),
        '\t' => (KEY_TAB, false),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_digits() {
        assert_eq!(android_to_evdev(29), Some(30)); // A
        assert_eq!(android_to_evdev(54), Some(44)); // Z
        assert_eq!(android_to_evdev(7), Some(11)); // 0
        assert_eq!(char_to_key('q'), Some((16, false)));
        assert_eq!(char_to_key('Q'), Some((16, true)));
        assert_eq!(char_to_key('?'), Some((53, true)));
        assert_eq!(char_to_key('é'), None);
    }

    #[test]
    fn keymap_is_embedded() {
        assert!(KEYMAP_US.starts_with("xkb_keymap"));
        assert!(KEYMAP_US.contains("modifier_map Mod4"));
    }
}
