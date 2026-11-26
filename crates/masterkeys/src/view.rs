//! What each key in the drawing should look like.

use masterkeys_core::devices::DeviceInfo;
use masterkeys_core::layout::Layout;
use masterkeys_core::bindings::{MAX_DELAY_MS, Macro, MacroMode, MacroStep};
use masterkeys_core::profile::ProfileSummary;
use std::collections::BTreeMap;
use steelseries_gg::input::InputKey;
use steelseries_gg::apex3::ZONE_COUNT;
use steelseries_gg::rgb::Color;

/// Each key's preview colour, in layout order, scaled by brightness (0-100). `frame` has one colour per
/// `device.leds`: on the Apex 3 a key shows its zone, elsewhere the LED under it (black if none).
pub fn key_colors(layout: &Layout, device: &DeviceInfo, frame: &[Color], brightness: u8) -> Vec<Color> {
    let scale = f32::from(brightness.min(100)) / 100.0;
    let at = |i: usize| frame.get(i).copied().unwrap_or(Color::BLACK);
    layout
        .keys
        .iter()
        .map(|k| match k.led {
            Some(led) => device.leds.iter().position(|l| l.index == led).map_or(Color::BLACK, at),
            None => at(usize::from(k.zone).min(ZONE_COUNT - 1)),
        })
        .map(|c| c.scale(scale))
        .collect()
}

pub fn key_name(code: u16) -> Option<&'static str> {
    InputKey::from_code(code).name()
}

/// What a key does in the active profile, for the Key panel.
pub fn binding_text(code: u16, remaps: &BTreeMap<String, String>, macros: &BTreeMap<String, Macro>) -> String {
    let Some(name) = key_name(code) else { return "Not remappable".into() };
    if let Some(target) = remaps.get(name) {
        return format!("Remapped to {target}");
    }
    if let Some(m) = macros.get(name) {
        let keys = m.steps.iter().filter(|s| !matches!(s, MacroStep::Delay(_))).count();
        let ms: u32 = m.steps.iter().map(|s| if let MacroStep::Delay(d) = s { *d } else { 0 }).sum();
        let repeat = match m.mode {
            MacroMode::Once => String::new(),
            MacroMode::WhileHeld => ", repeat while held".into(),
            MacroMode::Toggle => ", toggle repeat".into(),
            MacroMode::Count => format!(", {} times", m.count),
        };
        return format!("Macro: {keys} steps, {:.1} s{repeat}", f64::from(ms) / 1000.0);
    }
    "Default".into()
}

/// The playback modes as the Key panel lists them.
pub const MODE_LABELS: [&str; 4] = ["Once", "Repeat while held", "Toggle repeat", "Repeat N times"];
const MODES: [MacroMode; 4] = [MacroMode::Once, MacroMode::WhileHeld, MacroMode::Toggle, MacroMode::Count];

pub fn mode_index(mode: MacroMode) -> i32 {
    MODES.iter().position(|m| *m == mode).unwrap_or(0) as i32
}

pub fn mode_from_index(i: i32) -> MacroMode {
    usize::try_from(i).ok().and_then(|i| MODES.get(i)).copied().unwrap_or_default()
}

/// The keys pressed while recording, as macro text, built from the GUI's own key events (the
/// daemon's exact recording replaces it on Stop).
pub struct LiveRecording {
    steps: Vec<MacroStep>,
    last: Option<std::time::Instant>,
}

impl LiveRecording {
    pub fn start() -> Self {
        Self { steps: Vec::new(), last: None }
    }

    pub fn feed(&mut self, code: u16, pressed: bool, at: std::time::Instant) {
        let Some(name) = key_name(code) else { return };
        if let Some(last) = self.last {
            let ms = at.saturating_duration_since(last).as_millis().min(u128::from(MAX_DELAY_MS)) as u32;
            if ms >= 1 {
                self.steps.push(MacroStep::Delay(ms));
            }
        }
        self.last = Some(at);
        self.steps.push(if pressed { MacroStep::Down(name.into()) } else { MacroStep::Up(name.into()) });
    }

    pub fn text(&self) -> String {
        masterkeys_core::macro_text::to_text(&self.steps)
    }
}

/// Whether a key has a remap or macro, for the marker on the drawing.
pub fn marked(code: u16, remaps: &BTreeMap<String, String>, macros: &BTreeMap<String, Macro>) -> bool {
    key_name(code).is_some_and(|n| remaps.contains_key(n) || macros.contains_key(n))
}

/// Which profile the panel should select after the daemon sends a new list: the new name of a
/// rename once the list shows it done, else the current selection if it still exists, else the
/// active profile.
pub fn reselect(
    selected: Option<&str>,
    pending_rename: Option<&(String, String)>,
    list: &[ProfileSummary],
    active: &str,
) -> Option<String> {
    let exists = |name: &str| list.iter().any(|p| p.name == name);
    if let Some((old, new)) = pending_rename
        && selected == Some(old.as_str())
        && !exists(old)
        && exists(new)
    {
        return Some(new.clone());
    }
    selected.filter(|s| exists(s)).or(Some(active).filter(|a| exists(a))).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use masterkeys_core::bindings::{Macro, MacroMode, MacroStep};
    use masterkeys_core::profile::ProfileSummary;
    use std::collections::BTreeMap;

    fn summaries(names: &[&str]) -> Vec<ProfileSummary> {
        names.iter().map(|n| ProfileSummary { name: n.to_string(), executables: vec![], is_default: false }).collect()
    }

    fn binds() -> (BTreeMap<String, String>, BTreeMap<String, Macro>) {
        let remaps = BTreeMap::from([("caps_lock".to_string(), "esc".to_string())]);
        let tap = Macro {
            steps: vec![MacroStep::Down("h".into()), MacroStep::Delay(100), MacroStep::Up("h".into()), MacroStep::Delay(150)],
            ..Macro::default()
        };
        let macros = BTreeMap::from([("f1".to_string(), tap)]);
        (remaps, macros)
    }

    #[test]
    fn binding_texts() {
        let (remaps, macros) = binds();
        assert_eq!(binding_text(30, &remaps, &macros), "Default");
        assert_eq!(binding_text(58, &remaps, &macros), "Remapped to esc");
        assert_eq!(binding_text(59, &remaps, &macros), "Macro: 2 steps, 0.2 s");
        assert_eq!(binding_text(0, &remaps, &macros), "Not remappable");
    }

    #[test]
    fn binding_text_names_the_repeat_mode() {
        let (remaps, mut macros) = binds();
        macros.get_mut("f1").unwrap().mode = MacroMode::Toggle;
        assert_eq!(binding_text(59, &remaps, &macros), "Macro: 2 steps, 0.2 s, toggle repeat");
        let m = macros.get_mut("f1").unwrap();
        m.mode = MacroMode::Count;
        m.count = 5;
        assert_eq!(binding_text(59, &remaps, &macros), "Macro: 2 steps, 0.2 s, 5 times");
    }

    #[test]
    fn mode_list_round_trips() {
        for mode in [MacroMode::Once, MacroMode::WhileHeld, MacroMode::Toggle, MacroMode::Count] {
            assert_eq!(mode_from_index(mode_index(mode)), mode);
        }
        assert_eq!(MODE_LABELS.len(), 4);
        assert_eq!(mode_from_index(99), MacroMode::Once);
    }

    #[test]
    fn live_recording_text() {
        let t = std::time::Instant::now();
        let mut live = LiveRecording::start();
        live.feed(42, true, t);
        live.feed(35, true, t + std::time::Duration::from_millis(40));
        live.feed(35, false, t + std::time::Duration::from_millis(40));
        live.feed(42, false, t + std::time::Duration::from_millis(90));
        live.feed(9999, true, t);
        assert_eq!(live.text(), "down left_shift\nwait 40\ntap h\nwait 50\nup left_shift\n");
    }

    #[test]
    fn marks() {
        let (remaps, macros) = binds();
        assert!(marked(58, &remaps, &macros));
        assert!(marked(59, &remaps, &macros));
        assert!(!marked(30, &remaps, &macros));
    }

    #[test]
    fn selection_follows_a_rename_once_the_daemon_lists_it() {
        let pending = Some(("A".to_string(), "C".to_string()));
        assert_eq!(reselect(Some("A"), pending.as_ref(), &summaries(&["B", "C"]), "B").as_deref(), Some("C"));
    }

    #[test]
    fn selection_stays_put_while_a_rename_is_unconfirmed() {
        let pending = Some(("A".to_string(), "B".to_string()));
        assert_eq!(reselect(Some("A"), pending.as_ref(), &summaries(&["A", "B"]), "B").as_deref(), Some("A"));
    }

    #[test]
    fn deleted_selection_falls_back_to_the_active_profile() {
        assert_eq!(reselect(Some("A"), None, &summaries(&["B"]), "B").as_deref(), Some("B"));
    }

    use masterkeys_core::layout::Layout;
    use steelseries_gg::apex3::ZONE_COUNT;
    use steelseries_gg::rgb::Color;

    #[test]
    fn keys_take_their_zone_colour_scaled_by_brightness() {
        let l = Layout::apex3_ansi();
        let mut frame = [Color::BLACK; ZONE_COUNT];
        frame[0] = Color::new(200, 100, 0);
        let c = key_colors(&l, &DeviceInfo::apex3(), &frame, 50);
        let esc = l.keys.iter().position(|k| k.code == 1).unwrap();
        assert_eq!(c[esc], Color::new(100, 50, 0)); // Esc is in zone 0
        assert_eq!(c.len(), l.keys.len());
    }

    #[test]
    fn k745_keys_take_their_led_colour() {
        let d = DeviceInfo::k745pro();
        let l = d.layout();
        // One frame entry per LED, in `d.leds` order; LED 0 (Esc) is lit.
        let frame: Vec<Color> =
            d.leds.iter().map(|led| if led.index == 0 { Color::new(200, 100, 0) } else { Color::BLACK }).collect();
        let c = key_colors(&l, &d, &frame, 50);
        let esc = l.keys.iter().position(|k| k.code == 1).unwrap();
        assert_eq!(c[esc], Color::new(100, 50, 0));
        assert_eq!(c.len(), l.keys.len());
        assert_eq!(c.iter().filter(|&&x| x != Color::BLACK).count(), 1);
    }
}
