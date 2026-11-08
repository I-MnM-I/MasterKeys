//! Effect complexity scoring used by the RGB engine's adaptive frame timing.

use crate::rgb::Effect;

/// Effect complexity scoring for adaptive timing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectComplexity {
    /// Simple static effects - can run at lower framerates
    Simple,
    /// Medium complexity like breathing, spectrum - standard framerate
    Medium,
    /// High complexity like wave, reactive - needs consistent timing
    High,
    /// Critical effects requiring minimal latency - highest framerate
    Critical,
}

impl EffectComplexity {
    /// Get recommended frame budget for this complexity level.
    pub fn frame_budget_ms(&self) -> f32 {
        match self {
            EffectComplexity::Simple => 33.0,   // 30 FPS
            EffectComplexity::Medium => 16.67,  // 60 FPS
            EffectComplexity::High => 8.33,     // 120 FPS
            EffectComplexity::Critical => 4.17, // 240 FPS
        }
    }

    /// Get target FPS for this complexity level.
    pub fn target_fps(&self) -> f32 {
        1000.0 / self.frame_budget_ms()
    }
}

/// Determine effect complexity for adaptive timing.
pub fn calculate_effect_complexity(effect: &Effect) -> EffectComplexity {
    match effect {
        Effect::Static { .. } => EffectComplexity::Simple,
        Effect::Off => EffectComplexity::Simple,
        Effect::Custom { .. } => EffectComplexity::Simple,

        Effect::Breathing { .. } => EffectComplexity::Medium,
        Effect::Spectrum { .. } => EffectComplexity::Medium,
        Effect::Gradient { .. } => EffectComplexity::Medium,

        Effect::Wave { colors, .. } => {
            if colors.len() > 4 {
                EffectComplexity::High
            } else {
                EffectComplexity::Medium
            }
        }

        Effect::Reactive { .. } => EffectComplexity::Critical,
    }
}
