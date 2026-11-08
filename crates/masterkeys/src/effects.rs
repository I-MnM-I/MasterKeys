//! The effects the panel offers, and turning panel settings into an `Effect`.

use masterkeys_core::parse_hex;
use steelseries_gg::rgb::{Color, Effect, WaveDirection};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectKind {
    Static,
    Breathing,
    Spectrum,
    Wave,
    Gradient,
    Off,
}

impl EffectKind {
    pub const ALL: [EffectKind; 6] = [
        EffectKind::Static,
        EffectKind::Breathing,
        EffectKind::Spectrum,
        EffectKind::Wave,
        EffectKind::Gradient,
        EffectKind::Off,
    ];

    pub fn label(self) -> &'static str {
        match self {
            EffectKind::Static => "Static",
            EffectKind::Breathing => "Breathing",
            EffectKind::Spectrum => "Spectrum",
            EffectKind::Wave => "Wave",
            EffectKind::Gradient => "Gradient",
            EffectKind::Off => "Off",
        }
    }

    /// Whether the speed slider applies.
    pub fn has_speed(self) -> bool {
        matches!(self, EffectKind::Breathing | EffectKind::Spectrum | EffectKind::Wave)
    }
}

/// The effect for the panel's settings: colour slots A and B and a speed in cycles per second.
pub fn build(kind: EffectKind, a: Color, b: Color, speed: f32) -> Effect {
    match kind {
        EffectKind::Static => Effect::Static { color: a },
        EffectKind::Breathing => Effect::Breathing { color: a, speed },
        EffectKind::Spectrum => Effect::Spectrum { speed },
        EffectKind::Wave => Effect::Wave {
            colors: (0..6).map(|i| Color::from_hsv(i as f32 * 60.0, 1.0, 1.0)).collect(),
            speed,
            direction: WaveDirection::LeftToRight,
        },
        EffectKind::Gradient => Effect::Gradient { start: a, end: b },
        EffectKind::Off => Effect::Off,
    }
}

/// Which panel effect an `Effect` is; `None` for per-zone colours and effects the panel doesn't offer.
pub fn kind_of(effect: &Effect) -> Option<EffectKind> {
    match effect {
        Effect::Static { .. } => Some(EffectKind::Static),
        Effect::Breathing { .. } => Some(EffectKind::Breathing),
        Effect::Spectrum { .. } => Some(EffectKind::Spectrum),
        Effect::Wave { .. } => Some(EffectKind::Wave),
        Effect::Gradient { .. } => Some(EffectKind::Gradient),
        Effect::Off => Some(EffectKind::Off),
        Effect::Custom { .. } | Effect::Reactive { .. } => None,
    }
}

/// A colour typed into the hex field: `RRGGBB` or `#RRGGBB`, surrounding spaces ignored.
pub fn parse_color_input(text: &str) -> Option<Color> {
    parse_hex(text.trim()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use steelseries_gg::rgb::{Color, Effect};

    #[test]
    fn build_and_kind_round_trip() {
        for k in EffectKind::ALL {
            assert_eq!(kind_of(&build(k, Color::RED, Color::BLUE, 0.5)), Some(k));
        }
    }

    #[test]
    fn custom_has_no_kind() {
        assert_eq!(kind_of(&Effect::Custom { colors: vec![Color::RED; 10] }), None);
    }

    #[test]
    fn labels_are_distinct() {
        let labels: std::collections::HashSet<_> = EffectKind::ALL.iter().map(|k| k.label()).collect();
        assert_eq!(labels.len(), 6);
    }

    #[test]
    fn color_input() {
        assert_eq!(parse_color_input(" #ff8000 "), Some(Color::ORANGE));
        assert_eq!(parse_color_input("ff80"), None);
        assert_eq!(parse_color_input("orange"), None);
    }
}
