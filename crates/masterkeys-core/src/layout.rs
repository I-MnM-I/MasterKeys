//! The keyboard drawing: where each key sits and which lighting zone it belongs to.

use serde::Deserialize;

/// One key in the drawing. Positions and sizes are in key units (1 = one standard key).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct KeyCap {
    /// Linux evdev key code (`KEY_A` = 30).
    pub code: u16,
    pub label: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    #[serde(default = "one")]
    pub h: f32,
    /// Lighting zone, 0 (leftmost band) to 9; 0 on boards that light each key.
    #[serde(default)]
    pub zone: u8,
    /// Index of the key's LED in the lighting frame, on boards that light each key.
    #[serde(default)]
    pub led: Option<u16>,
}

fn one() -> f32 {
    1.0
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub keys: Vec<KeyCap>,
}

#[derive(Deserialize)]
struct LayoutFile {
    key: Vec<KeyCap>,
}

impl Layout {
    pub fn parse(toml: &str) -> Result<Layout, String> {
        let file: LayoutFile = toml::from_str(toml).map_err(|e| format!("bad layout file: {e}"))?;
        Ok(Layout { keys: file.key })
    }

    /// The bundled Apex 3 US ANSI layout.
    pub fn apex3_ansi() -> Layout {
        Self::parse(include_str!("../../../assets/layouts/apex3-ansi.toml")).expect("bundled layout is valid")
    }

    /// The bundled Redragon K745 PRO layout, generated from Redragon's KB.ini.
    pub fn k745pro() -> Layout {
        Self::parse(include_str!("../../../assets/layouts/k745pro.toml")).expect("bundled layout is valid")
    }

    pub fn width(&self) -> f32 {
        self.keys.iter().map(|k| k.x + k.w).fold(0.0, f32::max)
    }

    pub fn height(&self) -> f32 {
        self.keys.iter().map(|k| k.y + k.h).fold(0.0, f32::max)
    }

    pub fn by_code(&self, code: u16) -> Option<&KeyCap> {
        self.keys.iter().find(|k| k.code == code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_layout_is_full_ansi() {
        let l = Layout::apex3_ansi();
        assert_eq!(l.keys.len(), 104);
        let mut codes: Vec<u16> = l.keys.iter().map(|k| k.code).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), 104, "duplicate evdev codes");
    }

    #[test]
    fn every_zone_is_used_and_in_range() {
        let l = Layout::apex3_ansi();
        for z in 0..10u8 {
            assert!(l.keys.iter().any(|k| k.zone == z), "zone {z} unused");
        }
        assert!(l.keys.iter().all(|k| k.zone < 10));
    }

    #[test]
    fn keys_do_not_overlap() {
        let l = Layout::apex3_ansi();
        for (i, a) in l.keys.iter().enumerate() {
            for b in &l.keys[i + 1..] {
                let apart = a.x + a.w <= b.x + 0.01
                    || b.x + b.w <= a.x + 0.01
                    || a.y + a.h <= b.y + 0.01
                    || b.y + b.h <= a.y + 0.01;
                assert!(apart, "{} overlaps {}", a.label, b.label);
            }
        }
    }

    #[test]
    fn known_keys() {
        let l = Layout::apex3_ansi();
        assert_eq!(l.by_code(30).unwrap().label, "A");
        assert_eq!(l.by_code(1).unwrap().label, "Esc");
        assert_eq!(l.by_code(57).unwrap().w, 6.25);
        assert!(l.width() > 22.0 && l.width() < 23.0, "{}", l.width());
        assert_eq!(l.height(), 6.5);
    }

    #[test]
    fn k745_layout_has_unique_leds_and_esc_first() {
        let l = Layout::k745pro();
        assert!(l.keys.len() >= 104, "{} keys", l.keys.len());
        let mut leds: Vec<u16> = l.keys.iter().map(|k| k.led.expect("every K745 key has an LED")).collect();
        assert!(leds.iter().all(|&i| i < 126));
        leds.sort();
        leds.dedup();
        assert_eq!(leds.len(), l.keys.len(), "LED used twice");
        assert_eq!(l.by_code(1).unwrap().led, Some(0));
    }

    #[test]
    fn k745_layout_codes_are_unique() {
        let l = Layout::k745pro();
        let mut codes: Vec<u16> = l.keys.iter().map(|k| k.code).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), l.keys.len(), "duplicate evdev codes");
    }

    #[test]
    fn apex_layout_has_no_leds() {
        assert!(Layout::apex3_ansi().keys.iter().all(|k| k.led.is_none()));
    }

    #[test]
    fn parse_reports_bad_input() {
        assert!(Layout::parse("[[key]]\ncode = \"x\"").is_err());
    }
}
