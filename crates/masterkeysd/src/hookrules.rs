//! What the Windows keyboard hook does with each key event. Pure, so it is tested on every OS.

use std::collections::HashSet;
use std::time::Duration;

/// What the hook is for right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookMode {
    /// Nobody needs the keys.
    Off,
    /// Report keys (GUI, recording) but let Windows have them.
    Read,
    /// Take the keys: the engine replays them.
    Grab,
}

/// Past this age of the core's heartbeat the hook stops taking keys, so a stalled core never leaves
/// the keyboard dead.
pub const MAX_HEARTBEAT_AGE: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Let Windows have it; don't report it.
    Pass,
    /// Let Windows have it and report it (not grabbed).
    Report,
    /// Keep it from Windows and hand it to the engine (grabbed).
    Swallow,
    /// Keep it from Windows; nobody gets it.
    Drop,
}

/// `KBDLLHOOKSTRUCT.flags` bit for events some program sent rather than a keyboard.
pub const LLKHF_INJECTED: u32 = 0x10;
/// Marks the events MasterKeys sends.
pub const MASTERKEYS_TAG: usize = 0x4D4B_4559; // "MKEY"

/// Events a program sent (MasterKeys or another: auto-type, on-screen keyboard) always go to Windows
/// untouched; only physical keys are remapped, as on Linux.
pub fn is_injected(flags: u32, extra: usize) -> bool {
    flags & LLKHF_INJECTED != 0 || extra == MASTERKEYS_TAG
}

/// The core counts as alive only while the engine keeps up with the keys handed to it, so a stuck
/// engine (not just a stuck runtime) lets the keyboard go back to Windows.
pub fn should_beat(queued: usize) -> bool {
    queued == 0
}

/// Which keys are down; which of those Windows saw go down (so Windows must see them come up); and
/// which the engine took (so their repeats and release go to the engine, or nowhere, but never to
/// Windows).
#[derive(Debug, Default)]
pub struct HookState {
    down: HashSet<u16>,
    native: HashSet<u16>,
    swallowed: HashSet<u16>,
}

impl HookState {
    /// A swallowed key could not be handed to the engine, so the hook let Windows have it after all.
    pub fn not_delivered(&mut self, code: u16) {
        if self.swallowed.remove(&code) && self.down.contains(&code) {
            self.native.insert(code);
        }
    }

    /// Decide one key event. `ours` marks events MasterKeys injected; `heartbeat_age` is how long since
    /// the core last showed it is alive. Returns the action and the evdev value (0 up, 1 down, 2 repeat).
    pub fn on_key(&mut self, mode: HookMode, code: u16, down: bool, ours: bool, heartbeat_age: Duration) -> (Action, i32) {
        if ours {
            return (Action::Pass, i32::from(down));
        }
        let value = match (down, self.down.contains(&code)) {
            (true, true) => 2,
            (true, false) => 1,
            (false, _) => 0,
        };
        if down {
            self.down.insert(code);
        } else {
            self.down.remove(&code);
        }
        if self.swallowed.contains(&code) {
            if !down {
                self.swallowed.remove(&code);
            }
            let action = if mode == HookMode::Grab { Action::Swallow } else { Action::Drop };
            return (action, value);
        }
        let windows_has_it = self.native.contains(&code);
        let action = match mode {
            HookMode::Grab if heartbeat_age <= MAX_HEARTBEAT_AGE && !windows_has_it => Action::Swallow,
            HookMode::Off => Action::Pass,
            _ => Action::Report,
        };
        if action == Action::Swallow {
            self.native.remove(&code);
            if down {
                self.swallowed.insert(code);
            }
        } else if down {
            self.native.insert(code);
        } else {
            self.native.remove(&code);
        }
        (action, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRESH: Duration = Duration::from_millis(50);

    #[test]
    fn grab_swallows_physical_keys() {
        let mut h = HookState::default();
        assert_eq!(h.on_key(HookMode::Grab, 30, true, false, FRESH), (Action::Swallow, 1));
        assert_eq!(h.on_key(HookMode::Grab, 30, false, false, FRESH), (Action::Swallow, 0));
    }

    #[test]
    fn our_own_injected_keys_pass() {
        let mut h = HookState::default();
        assert_eq!(h.on_key(HookMode::Grab, 30, true, true, FRESH).0, Action::Pass);
        // and they don't count as held
        assert_eq!(h.on_key(HookMode::Grab, 30, true, false, FRESH), (Action::Swallow, 1));
    }

    #[test]
    fn read_reports_and_passes() {
        let mut h = HookState::default();
        assert_eq!(h.on_key(HookMode::Read, 30, true, false, FRESH), (Action::Report, 1));
    }

    #[test]
    fn off_passes() {
        let mut h = HookState::default();
        assert_eq!(h.on_key(HookMode::Off, 30, true, false, FRESH).0, Action::Pass);
    }

    #[test]
    fn stale_heartbeat_passes_through() {
        let mut h = HookState::default();
        assert_eq!(h.on_key(HookMode::Grab, 30, true, false, Duration::from_millis(1500)).0, Action::Report);
    }

    #[test]
    fn held_key_reports_repeat() {
        let mut h = HookState::default();
        assert_eq!(h.on_key(HookMode::Grab, 30, true, false, FRESH).1, 1);
        assert_eq!(h.on_key(HookMode::Grab, 30, true, false, FRESH).1, 2);
        assert_eq!(h.on_key(HookMode::Grab, 30, false, false, FRESH).1, 0);
        assert_eq!(h.on_key(HookMode::Grab, 30, true, false, FRESH).1, 1);
    }

    #[test]
    fn release_of_key_down_before_grab_passes() {
        let mut h = HookState::default();
        h.on_key(HookMode::Read, 28, true, false, FRESH);
        // Windows saw Enter go down, so it must see it come up.
        assert_eq!(h.on_key(HookMode::Grab, 28, false, false, FRESH), (Action::Report, 0));
        // and the next press is the engine's
        assert_eq!(h.on_key(HookMode::Grab, 28, true, false, FRESH), (Action::Swallow, 1));
    }

    #[test]
    fn anything_injected_passes() {
        assert!(is_injected(LLKHF_INJECTED, 0));
        assert!(is_injected(0, MASTERKEYS_TAG));
        assert!(!is_injected(0, 0));
    }

    #[test]
    fn stale_heartbeat_still_hands_releases_of_swallowed_keys_to_the_engine() {
        let mut h = HookState::default();
        assert_eq!(h.on_key(HookMode::Grab, 58, true, false, FRESH), (Action::Swallow, 1));
        let stale = Duration::from_millis(1500);
        assert_eq!(h.on_key(HookMode::Grab, 58, true, false, stale), (Action::Swallow, 2));
        assert_eq!(h.on_key(HookMode::Grab, 58, false, false, stale), (Action::Swallow, 0));
    }

    #[test]
    fn a_swallowed_key_stays_hidden_after_the_grab_ends() {
        let mut h = HookState::default();
        h.on_key(HookMode::Grab, 58, true, false, FRESH);
        // Windows never saw Caps Lock go down, so its repeats and release must not reach it either.
        assert_eq!(h.on_key(HookMode::Off, 58, true, false, FRESH), (Action::Drop, 2));
        assert_eq!(h.on_key(HookMode::Read, 58, false, false, FRESH), (Action::Drop, 0));
        assert_eq!(h.on_key(HookMode::Off, 58, true, false, FRESH), (Action::Pass, 1));
    }

    #[test]
    fn a_swallow_that_could_not_be_delivered_belongs_to_windows() {
        let mut h = HookState::default();
        h.on_key(HookMode::Grab, 30, true, false, FRESH);
        h.not_delivered(30);
        assert_eq!(h.on_key(HookMode::Grab, 30, false, false, FRESH), (Action::Report, 0));
    }

    #[test]
    fn beats_only_while_the_engine_keeps_up() {
        assert!(should_beat(0));
        assert!(!should_beat(3));
    }

    #[test]
    fn repeats_of_a_key_windows_holds_stay_with_windows() {
        let mut h = HookState::default();
        h.on_key(HookMode::Read, 28, true, false, FRESH);
        assert_eq!(h.on_key(HookMode::Grab, 28, true, false, FRESH), (Action::Report, 2));
    }
}
