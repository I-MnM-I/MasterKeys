//! Turns the lighting state into zone frames over time.

use crate::LightingState;
use std::time::Duration;
use steelseries_gg::apex3::ZONE_COUNT;
use steelseries_gg::rgb::{Color, Effect, EffectEngine};

/// Shortest gap between animated frames (about 60 fps), whatever the engine asks for.
pub const MIN_FRAME: Duration = Duration::from_millis(16);

/// Produces frames for the current effect. Brightness is not applied here; it goes to hardware.
pub struct Renderer {
    engine: EffectEngine,
    current: [Color; ZONE_COUNT],
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub fn new() -> Self {
        let state = LightingState::default();
        Self { engine: EffectEngine::new(state.effect, ZONE_COUNT), current: [Color::BLACK; ZONE_COUNT] }
    }

    pub fn set_state(&mut self, state: &LightingState) {
        self.engine.set_effect(state.effect.clone());
    }

    /// Compute the frame for now.
    pub fn frame(&mut self) -> [Color; ZONE_COUNT] {
        let colors = self.engine.compute();
        let last = colors.last().copied().unwrap_or(Color::BLACK);
        self.current = std::array::from_fn(|i| colors.get(i).copied().unwrap_or(last));
        self.current
    }

    /// The last frame produced.
    pub fn current_colors(&self) -> [Color; ZONE_COUNT] {
        self.current
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
    use steelseries_gg::apex3::ZONE_COUNT;
    use steelseries_gg::rgb::{Color, Effect};

    fn state(effect: Effect) -> LightingState {
        LightingState { effect, brightness: 100 }
    }

    #[test]
    fn static_fills_all_zones_and_does_not_animate() {
        let mut r = Renderer::new();
        r.set_state(&state(Effect::Static { color: Color::RED }));
        assert_eq!(r.frame(), [Color::RED; ZONE_COUNT]);
        assert_eq!(r.next_frame_in(), None);
    }

    #[test]
    fn custom_maps_zone_by_zone() {
        let colors: Vec<Color> = (0..10).map(|i| Color::new(i * 10, 0, 0)).collect();
        let mut r = Renderer::new();
        r.set_state(&state(Effect::Custom { colors: colors.clone() }));
        assert_eq!(r.frame().to_vec(), colors);
        assert_eq!(r.next_frame_in(), None);
    }

    #[test]
    fn off_is_black() {
        let mut r = Renderer::new();
        r.set_state(&state(Effect::Off));
        assert_eq!(r.frame(), [Color::BLACK; ZONE_COUNT]);
        assert_eq!(r.next_frame_in(), None);
    }

    #[test]
    fn animated_effects_are_capped_at_60fps() {
        let mut r = Renderer::new();
        r.set_state(&state(Effect::Spectrum { speed: 1.0 }));
        assert!(r.next_frame_in().unwrap() >= MIN_FRAME);
    }

    #[test]
    fn current_colors_tracks_last_frame() {
        let mut r = Renderer::new();
        r.set_state(&state(Effect::Static { color: Color::BLUE }));
        r.frame();
        assert_eq!(r.current_colors(), [Color::BLUE; ZONE_COUNT]);
    }
}
