//! The lighting devices MasterKeys can drive, each described as a list of LEDs with positions.

use crate::layout::Layout;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceKind {
    Keyboard,
    Mouse,
}

/// One LED: its slot in the device's frame, the key it sits under (evdev code) and its position,
/// 0.0 (left / top) to 1.0 (right / bottom) across the device.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Led {
    pub index: u16,
    pub key: Option<u16>,
    pub x: f32,
    pub y: f32,
}

/// A lighting device model. `strip` is how many columns effects are drawn at before each LED
/// takes the column under it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    pub strip: usize,
    pub leds: Vec<Led>,
}

pub const APEX3_ID: &str = "apex3";
pub const K745_ID: &str = "k745pro";

impl DeviceInfo {
    /// The SteelSeries Apex 3: ten zones, left to right, each lit as one LED at its centre.
    pub fn apex3() -> DeviceInfo {
        let zones = steelseries_gg::apex3::ZONE_COUNT;
        DeviceInfo {
            id: APEX3_ID.into(),
            name: "SteelSeries Apex 3".into(),
            kind: DeviceKind::Keyboard,
            strip: zones,
            leds: (0..zones)
                .map(|i| Led { index: i as u16, key: None, x: (i as f32 + 0.5) / zones as f32, y: 0.5 })
                .collect(),
        }
    }

    /// The Redragon K745 PRO on its cable: one LED under every key, from its bundled layout.
    pub fn k745pro() -> DeviceInfo {
        let layout = Layout::k745pro();
        let (w, h) = (layout.width(), layout.height());
        let leds = layout
            .keys
            .iter()
            .filter_map(|k| {
                Some(Led { index: k.led?, key: Some(k.code), x: (k.x + k.w / 2.0) / w, y: (k.y + k.h / 2.0) / h })
            })
            .collect();
        DeviceInfo { id: K745_ID.into(), name: "Redragon K745 PRO".into(), kind: DeviceKind::Keyboard, strip: 22, leds }
    }

    pub fn by_id(id: &str) -> Option<DeviceInfo> {
        match id {
            APEX3_ID => Some(Self::apex3()),
            K745_ID => Some(Self::k745pro()),
            _ => None,
        }
    }

    /// The keyboard picture for this device.
    pub fn layout(&self) -> Layout {
        match self.id.as_str() {
            K745_ID => Layout::k745pro(),
            _ => Layout::apex3_ansi(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apex3_has_ten_zone_centres() {
        let d = DeviceInfo::apex3();
        assert_eq!(d.leds.len(), 10);
        assert_eq!(d.strip, 10);
        assert!((d.leds[3].x - 0.35).abs() < 1e-6);
        assert_eq!(d.leds[3].index, 3);
        assert!(d.leds.iter().all(|l| l.key.is_none()));
    }

    #[test]
    fn k745_leds_cover_the_board() {
        let d = DeviceInfo::k745pro();
        let min = d.leds.iter().map(|l| l.x).fold(f32::MAX, f32::min);
        let max = d.leds.iter().map(|l| l.x).fold(f32::MIN, f32::max);
        assert!(min < 0.05 && max > 0.95, "{min}..{max}");
        assert!(d.leds.iter().all(|l| (0.0..=1.0).contains(&l.x) && (0.0..=1.0).contains(&l.y)));
        let mut idx: Vec<u16> = d.leds.iter().map(|l| l.index).collect();
        idx.sort();
        idx.dedup();
        assert_eq!(idx.len(), d.leds.len());
        let esc = d.leds.iter().find(|l| l.key == Some(1)).unwrap();
        assert_eq!(esc.index, 0);
    }

    #[test]
    fn by_id_round_trips() {
        for d in [DeviceInfo::apex3(), DeviceInfo::k745pro()] {
            assert_eq!(DeviceInfo::by_id(&d.id), Some(d.clone()));
            assert!(!d.layout().keys.is_empty());
        }
        assert_eq!(DeviceInfo::by_id("nope"), None);
    }
}
