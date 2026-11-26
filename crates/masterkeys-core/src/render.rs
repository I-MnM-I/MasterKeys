//! Turns the lighting state into one frame per device over time.

use crate::LightingState;
use crate::devices::{APEX3_ID, DeviceInfo};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use steelseries_gg::rgb::{Color, Effect, EffectEngine};

/// Shortest gap between animated frames (about 60 fps), whatever the engine asks for.
pub const MIN_FRAME: Duration = Duration::from_millis(16);

/// Produces frames for one device: the effect is drawn along a strip of `device.strip` columns and
/// each LED takes the column under it. Brightness is not applied here; it goes to the driver.
pub struct Renderer {
    device: DeviceInfo,
    engine: EffectEngine,
    started: Instant,
    custom: BTreeMap<u16, Color>,
    current: Vec<Color>,
}

impl Renderer {
    pub fn new(device: &DeviceInfo) -> Self {
        let state = LightingState::default();
        Self {
            device: device.clone(),
            engine: EffectEngine::new(state.effect, device.strip.max(1)),
            started: Instant::now(),
            custom: BTreeMap::new(),
            current: vec![Color::BLACK; device.leds.len()],
        }
    }

    pub fn device(&self) -> &DeviceInfo {
        &self.device
    }

    pub fn set_state(&mut self, state: &LightingState) {
        if self.engine.effect() != &state.effect {
            self.engine.set_effect(state.effect.clone());
            self.started = Instant::now();
        }
        self.custom = state.custom.get(&self.device.id).cloned().unwrap_or_default();
    }

    /// Compute the frame for now.
    pub fn frame(&mut self) -> Vec<Color> {
        self.frame_at(self.started.elapsed())
    }

    /// Compute the frame as it is `elapsed` after the effect started.
    pub fn frame_at(&mut self, elapsed: Duration) -> Vec<Color> {
        let per_key = self.device.id != APEX3_ID;
        self.current = match self.engine.effect() {
            // A device with its own key colours shows them (keys without one are dark).
            Effect::Custom { .. } if per_key && !self.custom.is_empty() => {
                self.device.leds.iter().map(|l| self.custom.get(&l.index).copied().unwrap_or(Color::BLACK)).collect()
            }
            // Without any, it shows the Apex 3's zone colours across the board.
            Effect::Custom { colors } if per_key => {
                let last = colors.len().saturating_sub(1);
                let n = colors.len() as f32;
                self.device
                    .leds
                    .iter()
                    .map(|l| colors.get(((l.x * n) as usize).min(last)).copied().unwrap_or(Color::BLACK))
                    .collect()
            }
            _ => {
            let strip = self.engine.compute_at(elapsed);
            let last = strip.len().saturating_sub(1);
            let n = self.device.strip.max(1) as f32;
            self.device
                .leds
                .iter()
                    .map(|l| strip.get(((l.x * n) as usize).min(last)).copied().unwrap_or(Color::BLACK))
                    .collect()
            }
        };
        self.current.clone()
    }

    /// The last frame produced.
    pub fn current_colors(&self) -> Vec<Color> {
        self.current.clone()
    }

    /// How long until the next frame is due; `None` when the effect does not change over time.
    pub fn next_frame_in(&self) -> Option<Duration> {
        match self.engine.effect() {
            Effect::Static { .. } | Effect::Custom { .. } | Effect::Off | Effect::Gradient { .. } => None,
            _ => Some(self.engine.calculate_frame_budget().max(MIN_FRAME)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LightingState;
    use crate::devices::{DeviceInfo, K745_ID};
    use std::collections::BTreeMap;
    use steelseries_gg::apex3::ZONE_COUNT;
    use steelseries_gg::rgb::{Color, Effect, EffectEngine, WaveDirection};

    fn state(effect: Effect) -> LightingState {
        LightingState { effect, brightness: 100, custom: BTreeMap::new() }
    }

    fn apex() -> Renderer {
        Renderer::new(&DeviceInfo::apex3())
    }

    #[test]
    fn static_fills_all_zones_and_does_not_animate() {
        let mut r = apex();
        r.set_state(&state(Effect::Static { color: Color::RED }));
        assert_eq!(r.frame(), vec![Color::RED; ZONE_COUNT]);
        assert_eq!(r.next_frame_in(), None);
    }

    #[test]
    fn custom_maps_zone_by_zone() {
        let colors: Vec<Color> = (0..10).map(|i| Color::new(i * 10, 0, 0)).collect();
        let mut r = apex();
        r.set_state(&state(Effect::Custom { colors: colors.clone() }));
        assert_eq!(r.frame(), colors);
        assert_eq!(r.next_frame_in(), None);
    }

    #[test]
    fn off_is_black() {
        let mut r = apex();
        r.set_state(&state(Effect::Off));
        assert_eq!(r.frame(), vec![Color::BLACK; ZONE_COUNT]);
        assert_eq!(r.next_frame_in(), None);
    }

    #[test]
    fn animated_effects_are_capped_at_60fps() {
        let mut r = apex();
        r.set_state(&state(Effect::Spectrum { speed: 1.0 }));
        assert!(r.next_frame_in().unwrap() >= MIN_FRAME);
    }

    #[test]
    fn current_colors_tracks_last_frame() {
        let mut r = apex();
        r.set_state(&state(Effect::Static { color: Color::BLUE }));
        r.frame();
        assert_eq!(r.current_colors(), vec![Color::BLUE; ZONE_COUNT]);
    }

    #[test]
    fn apex3_frames_match_the_zone_engine() {
        let effects = [
            Effect::Static { color: Color::RED },
            Effect::Breathing { color: Color::GREEN, speed: 0.7 },
            Effect::Spectrum { speed: 0.3 },
            Effect::Wave { colors: vec![Color::RED, Color::BLUE], speed: 0.5, direction: WaveDirection::LeftToRight },
            Effect::Wave { colors: vec![Color::RED, Color::BLUE], speed: 0.5, direction: WaveDirection::RightToLeft },
            Effect::Gradient { start: Color::RED, end: Color::BLUE },
            Effect::Custom { colors: (0..10).map(|i| Color::new(i * 20, 5, 9)).collect() },
            Effect::Off,
        ];
        let at = Duration::from_millis(1234);
        for effect in effects {
            let mut r = apex();
            r.set_state(&state(effect.clone()));
            let want = EffectEngine::new(effect.clone(), ZONE_COUNT).compute_at(at).to_vec();
            assert_eq!(r.frame_at(at), want, "{effect:?}");
        }
    }

    #[test]
    fn wave_varies_along_x_on_k745() {
        let d = DeviceInfo::k745pro();
        let mut r = Renderer::new(&d);
        r.set_state(&state(Effect::Gradient { start: Color::RED, end: Color::BLUE }));
        let frame = r.frame();
        let esc = d.leds.iter().position(|l| l.key == Some(1)).unwrap();
        let right = (0..d.leds.len()).max_by(|&a, &b| d.leds[a].x.total_cmp(&d.leds[b].x)).unwrap();
        assert_eq!(frame.len(), d.leds.len());
        assert_ne!(frame[esc], frame[right]);
    }

    #[test]
    fn custom_k745_uses_the_key_map() {
        let d = DeviceInfo::k745pro();
        let mut s = state(Effect::Custom { colors: vec![Color::GREEN; ZONE_COUNT] });
        s.custom.insert(K745_ID.into(), BTreeMap::from([(0u16, Color::RED)]));
        let mut r = Renderer::new(&d);
        r.set_state(&s);
        let frame = r.frame();
        for (led, color) in d.leds.iter().zip(&frame) {
            let want = if led.index == 0 { Color::RED } else { Color::BLACK };
            assert_eq!(*color, want, "led {}", led.index);
        }
    }

    #[test]
    fn k745_without_key_map_follows_apex_zones_under_custom() {
        let d = DeviceInfo::k745pro();
        let zones: Vec<Color> = (0..10).map(|i| Color::new(i * 25, 0, 0)).collect();
        let mut r = Renderer::new(&d);
        r.set_state(&state(Effect::Custom { colors: zones.clone() }));
        let frame = r.frame();
        let esc = d.leds.iter().position(|l| l.key == Some(1)).unwrap();
        let right = (0..d.leds.len()).max_by(|&a, &b| d.leds[a].x.total_cmp(&d.leds[b].x)).unwrap();
        assert_eq!(frame[esc], zones[0]);
        assert_eq!(frame[right], zones[9]);
    }
}
