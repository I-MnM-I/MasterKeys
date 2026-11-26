//! Windows key codes ↔ the evdev codes the engine, layout and profiles use, so bindings mean the same
//! keys on both systems. Pure tables, compiled everywhere so they are tested on Linux too.

/// How to send a key with SendInput.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Out {
    /// Set-1 scan code, and whether it carries the E0 prefix.
    Scan(u16, bool),
    /// Virtual-key code, for keys without a usable scan code.
    Vk(u16),
}

/// E0-prefixed scan codes: (scan, evdev code).
const EXTENDED: [(u16, u16); 18] = [
    (0x1C, 96),  // keypad Enter
    (0x1D, 97),  // right Ctrl
    (0x35, 98),  // keypad /
    (0x37, 99),  // Print Screen
    (0x38, 100), // right Alt
    (0x47, 102), // Home
    (0x48, 103), // Up
    (0x49, 104), // Page Up
    (0x4B, 105), // Left
    (0x4D, 106), // Right
    (0x4F, 107), // End
    (0x50, 108), // Down
    (0x51, 109), // Page Down
    (0x52, 110), // Insert
    (0x53, 111), // Delete
    (0x5B, 125), // left Windows
    (0x5C, 126), // right Windows
    (0x5D, 127), // Menu
];

/// Keys matched by virtual key: (vk, evdev code).
const BY_VK: [(u16, u16); 12] = [
    (0x13, 119), // Pause
    (0x90, 69),  // Num Lock
    (0xAD, 113), // mute
    (0xAE, 114), // volume down
    (0xAF, 115), // volume up
    (0xB0, 163), // next track
    (0xB1, 165), // previous track
    (0xB2, 166), // stop
    (0xB3, 164), // play/pause
    (0xB4, 155), // mail
    (0xB5, 226), // media
    (0xB7, 140), // calculator
];

/// Set-1 scan codes 0x01–0x58 are the evdev codes themselves (0x54–0x55 are unused).
pub fn scan_to_code(scan: u16, extended: bool) -> Option<u16> {
    if extended {
        EXTENDED.iter().find(|(s, _)| *s == scan).map(|(_, c)| *c)
    } else {
        matches!(scan, 0x01..=0x53 | 0x56..=0x58).then_some(scan)
    }
}

pub fn code_to_scan(code: u16) -> Option<(u16, bool)> {
    if let Some((s, _)) = EXTENDED.iter().find(|(_, c)| *c == code) {
        return Some((*s, true));
    }
    scan_to_code(code, false).map(|s| (s, false))
}

pub fn vk_to_code(vk: u16) -> Option<u16> {
    BY_VK.iter().find(|(v, _)| *v == vk).map(|(_, c)| *c)
}

pub fn code_to_vk(code: u16) -> Option<u16> {
    BY_VK.iter().find(|(_, c)| *c == code).map(|(v, _)| *v)
}

/// The evdev code for a hook event: by virtual key where the scan code is ambiguous or missing, else
/// by scan code.
pub fn hook_code(vk: u16, scan: u16, extended: bool) -> Option<u16> {
    vk_to_code(vk).or_else(|| scan_to_code(scan, extended))
}

/// How to send `code`: by virtual key for the keys the hook matches that way, else by scan code.
pub fn send_as(code: u16) -> Option<Out> {
    code_to_vk(code).map(Out::Vk).or_else(|| code_to_scan(code).map(|(s, e)| Out::Scan(s, e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use masterkeys_core::layout::Layout;

    #[test]
    fn every_layout_key_round_trips() {
        for key in &Layout::apex3_ansi().keys {
            let back = match send_as(key.code) {
                Some(Out::Scan(scan, extended)) => scan_to_code(scan, extended),
                Some(Out::Vk(vk)) => vk_to_code(vk),
                None => None,
            };
            assert_eq!(back, Some(key.code), "{} ({})", key.label, key.code);
        }
    }

    #[test]
    fn extended_keys_differ() {
        assert_eq!(scan_to_code(0x1C, false), Some(28));
        assert_eq!(scan_to_code(0x1C, true), Some(96));
        assert_eq!(scan_to_code(0x48, true), Some(103));
        assert_eq!(scan_to_code(0x2A, false), Some(42));
        assert_eq!(scan_to_code(0x1D, true), Some(97));
        assert_eq!(scan_to_code(0x38, true), Some(100));
    }

    #[test]
    fn media_vks_map() {
        assert_eq!(vk_to_code(0xAD), Some(113));
        assert_eq!(vk_to_code(0xAE), Some(114));
        assert_eq!(vk_to_code(0xAF), Some(115));
        assert_eq!(vk_to_code(0xB3), Some(164));
    }

    #[test]
    fn unknown_codes_are_none() {
        assert_eq!(scan_to_code(0x7F, true), None);
        assert_eq!(scan_to_code(0x54, false), None);
        assert_eq!(send_as(500), None);
    }

    #[test]
    fn pause_and_num_lock_go_by_virtual_key() {
        // The hook reports both as scan code 0x45, told apart only by the extended flag (inverted from
        // the hardware's), so they are matched by virtual key instead.
        assert_eq!(hook_code(0x13, 0x45, false), Some(119));
        assert_eq!(hook_code(0x90, 0x45, true), Some(69));
        assert_eq!(hook_code(0x41, 0x1E, false), Some(30));
    }
}
