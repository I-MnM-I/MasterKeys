//! Lighting state: the effect plus hardware brightness.
//!
//! Per-zone colours are `Effect::Custom { colors }` and one colour is `Effect::Static`, so the colours
//! live in exactly one place.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use steelseries_gg::apex3::ZONE_COUNT;
use steelseries_gg::rgb::{Color, Effect};

/// Highest LED count of any supported device's frame.
pub const MAX_LEDS: u16 = 126;

/// What the keyboard should show.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LightingState {
    pub effect: Effect,
    /// Hardware brightness, 0-100.
    pub brightness: u8,
    /// Per-key custom colours for devices other than the Apex 3: device id, then LED index. Shown
    /// while the effect is `Custom` (whose `colors` stay the Apex 3's ten zones).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub custom: BTreeMap<String, BTreeMap<u16, Color>>,
}

impl Default for LightingState {
    fn default() -> Self {
        Self { effect: Effect::Static { color: Color::WHITE }, brightness: 100, custom: BTreeMap::new() }
    }
}

impl LightingState {
    /// Check the state can be sent to the keyboard.
    pub fn validate(&self) -> Result<(), String> {
        if self.brightness > 100 {
            return Err(format!("brightness must be 0-100, got {}", self.brightness));
        }
        if let Some((id, led)) =
            self.custom.iter().find_map(|(id, leds)| leds.keys().find(|&&l| l >= MAX_LEDS).map(|l| (id, *l)))
        {
            return Err(format!("{id}: LED {led} is out of range (0-{})", MAX_LEDS - 1));
        }
        match &self.effect {
            Effect::Custom { colors } if colors.len() != ZONE_COUNT => {
                Err(format!("custom lighting needs {ZONE_COUNT} zone colours, got {}", colors.len()))
            }
            Effect::Reactive { .. } => Err("reactive lighting arrives with key remapping (milestone 5)".into()),
            _ => Ok(()),
        }
    }
}

/// The state plus whether the keyboard is connected, as `GetState` and `StateChanged` report it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StateReport {
    #[serde(flatten)]
    pub state: LightingState,
    pub connected: bool,
    /// Ids of the connected lighting devices, sorted.
    #[serde(default)]
    pub devices: Vec<String>,
    /// Name of the active profile.
    #[serde(default)]
    pub profile: String,
    /// The active profile's remaps and macros.
    #[serde(default)]
    pub remaps: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub macros: std::collections::BTreeMap<String, crate::bindings::Macro>,
    #[serde(default)]
    pub input: InputStatus,
    #[serde(default)]
    pub gamesense: GameSenseStatus,
}

/// GameSense: on or off, the game colouring the keyboard now, and the server's port (0 = not serving).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameSenseStatus {
    pub enabled: bool,
    pub game: Option<String>,
    pub port: u16,
}

impl Default for GameSenseStatus {
    fn default() -> Self {
        Self { enabled: true, game: None, port: 0 }
    }
}

/// What the daemon is doing with the keyboard's keys.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "message")]
pub enum InputStatus {
    /// Nothing needs the keys.
    #[default]
    Off,
    /// Reading keys for the GUI or a macro recording, without remapping.
    Reading,
    /// The keyboard is grabbed and replayed through the virtual keyboard.
    Remapping,
    /// Remapping was stopped (escape chord) until the bindings change.
    Stopped(String),
    /// Remapping can't run, e.g. no permission for /dev/uinput.
    Unavailable(String),
}

/// Parse `RRGGBB` or `#RRGGBB`.
pub fn parse_hex(color: &str) -> Result<Color, String> {
    let hex = color.strip_prefix('#').unwrap_or(color);
    let bad = || format!("not a colour (expected RRGGBB): {color}");
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad());
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| bad());
    Ok(Color::new(channel(0)?, channel(2)?, channel(4)?))
}

/// Lowercase `rrggbb`.
pub fn to_hex(color: Color) -> String {
    format!("{:02x}{:02x}{:02x}", color.r, color.g, color.b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use steelseries_gg::rgb::{Color, Effect};

    #[test]
    fn default_is_white_full_brightness() {
        let s = LightingState::default();
        assert_eq!(s.effect, Effect::Static { color: Color::WHITE });
        assert_eq!(s.brightness, 100);
    }

    #[test]
    fn json_round_trip() {
        let s = LightingState { effect: Effect::Custom { colors: vec![Color::RED; 10] }, brightness: 40, ..Default::default() };
        let back: LightingState = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn report_flattens_state() {
        let r = StateReport { state: LightingState::default(), connected: true, profile: "Default".into(), ..Default::default() };
        let v: serde_json::Value = serde_json::to_value(&r).unwrap();
        assert_eq!(v["brightness"], 100);
        assert_eq!(v["connected"], true);
    }

    #[test]
    fn old_reports_parse_without_bindings() {
        let r: StateReport = serde_json::from_str(r#"{"effect":"Off","brightness":50,"connected":true}"#).unwrap();
        assert!(r.remaps.is_empty() && r.macros.is_empty());
        assert_eq!(r.input, InputStatus::Off);
    }

    #[test]
    fn input_status_serializes_tagged() {
        let v = serde_json::to_value(InputStatus::Unavailable("no uinput".into())).unwrap();
        assert_eq!(v, serde_json::json!({"kind": "Unavailable", "message": "no uinput"}));
    }

    #[test]
    fn validate_rejects_bad_states() {
        let bad = |effect, brightness| LightingState { effect, brightness, ..Default::default() }.validate().is_err();
        assert!(bad(Effect::Off, 101));
        assert!(bad(Effect::Custom { colors: vec![Color::RED; 3] }, 50));
        assert!(bad(Effect::Reactive { color: Color::RED, duration: 1.0 }, 50));
        assert!(LightingState::default().validate().is_ok());
    }

    #[test]
    fn old_profile_loads_and_new_keeps_old_fields() {
        let old = serde_json::to_string(&serde_json::json!({
            "effect": {"Custom": {"colors": vec![Color::RED; 10]}}, "brightness": 80
        }))
        .unwrap();
        let s: LightingState = serde_json::from_str(&old).unwrap();
        assert!(s.custom.is_empty());
        assert_eq!(s.effect, Effect::Custom { colors: vec![Color::RED; 10] });
        let v = serde_json::to_value(&s).unwrap();
        assert!(v.get("custom").is_none(), "{v}");
        assert_eq!(v["brightness"], 80);
    }

    #[test]
    fn custom_rejects_out_of_range_led() {
        let mut s = LightingState::default();
        s.custom.insert("k745pro".into(), BTreeMap::from([(126u16, Color::RED)]));
        assert!(s.validate().is_err());
        s.custom.insert("k745pro".into(), BTreeMap::from([(125u16, Color::RED)]));
        assert!(s.validate().is_ok());
    }

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("ff8000").unwrap(), Color::ORANGE);
        assert_eq!(parse_hex("#FF8000").unwrap(), Color::ORANGE);
        assert!(parse_hex("zz0000").unwrap_err().contains("zz0000"));
        assert!(parse_hex("red").is_err());
        assert_eq!(to_hex(Color::ORANGE), "ff8000");
    }
}
