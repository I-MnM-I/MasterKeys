//! RGB lighting control and effects.

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// RGB color representation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    /// Create a new color from RGB values.
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Create color from hex value (0xRRGGBB).
    pub const fn from_hex(hex: u32) -> Self {
        Self {
            r: ((hex >> 16) & 0xFF) as u8,
            g: ((hex >> 8) & 0xFF) as u8,
            b: (hex & 0xFF) as u8,
        }
    }

    /// Convert to hex value.
    pub const fn to_hex(&self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }

    /// Parse a colour name (`red`, `off`, ...) or a `RRGGBB` / `#RRGGBB` hex string.
    pub fn parse(s: &str) -> Option<Color> {
        const NAMED: [(&str, Color); 12] = [
            ("red", Color::RED),
            ("green", Color::GREEN),
            ("blue", Color::BLUE),
            ("white", Color::WHITE),
            ("black", Color::BLACK),
            ("off", Color::BLACK),
            ("cyan", Color::CYAN),
            ("magenta", Color::MAGENTA),
            ("yellow", Color::YELLOW),
            ("orange", Color::ORANGE),
            ("purple", Color::PURPLE),
            ("pink", Color::PINK),
        ];
        if let Some((_, color)) = NAMED.iter().find(|(name, _)| s.eq_ignore_ascii_case(name)) {
            return Some(*color);
        }

        let hex = s.trim_start_matches('#');
        if hex.len() == 6
            && let Ok(val) = u32::from_str_radix(hex, 16)
        {
            return Some(Color::from_hex(val));
        }

        None
    }

    /// Black (off).
    pub const BLACK: Color = Color::new(0, 0, 0);
    /// White.
    pub const WHITE: Color = Color::new(255, 255, 255);
    /// Red.
    pub const RED: Color = Color::new(255, 0, 0);
    /// Green.
    pub const GREEN: Color = Color::new(0, 255, 0);
    /// Blue.
    pub const BLUE: Color = Color::new(0, 0, 255);
    /// Cyan.
    pub const CYAN: Color = Color::new(0, 255, 255);
    /// Magenta.
    pub const MAGENTA: Color = Color::new(255, 0, 255);
    /// Yellow.
    pub const YELLOW: Color = Color::new(255, 255, 0);
    /// Orange.
    pub const ORANGE: Color = Color::new(255, 128, 0);
    /// Purple.
    pub const PURPLE: Color = Color::new(128, 0, 255);
    /// Pink.
    pub const PINK: Color = Color::new(255, 105, 180);

    /// Default 7-color sequence for wave effects.
    pub const DEFAULT_COLORS: [Color; 7] = [
        Color::RED,
        Color::ORANGE,
        Color::YELLOW,
        Color::GREEN,
        Color::CYAN,
        Color::BLUE,
        Color::PURPLE,
    ];

    /// Standard 8-color rainbow sequence.
    pub const RAINBOW_COLORS: [Color; 8] = [
        Color::RED,
        Color::ORANGE,
        Color::YELLOW,
        Color::GREEN,
        Color::CYAN,
        Color::BLUE,
        Color::PURPLE,
        Color::MAGENTA,
    ];

    /// Blend between two colors.
    #[inline]
    pub fn blend(a: Color, b: Color, t: f32) -> Color {
        let t = t.clamp(0.0, 1.0);
        let inv_t = 1.0 - t; // Pre-calculate inverse

        // Use more efficient blending with fewer operations
        Color {
            r: (a.r as f32 * inv_t + b.r as f32 * t) as u8,
            g: (a.g as f32 * inv_t + b.g as f32 * t) as u8,
            b: (a.b as f32 * inv_t + b.b as f32 * t) as u8,
        }
    }

    /// Scale brightness (0.0 = black, 1.0 = original).
    #[inline]
    pub fn scale(&self, factor: f32) -> Color {
        let factor = factor.clamp(0.0, 1.0);
        Color {
            r: (self.r as f32 * factor) as u8,
            g: (self.g as f32 * factor) as u8,
            b: (self.b as f32 * factor) as u8,
        }
    }

    /// Convert from HSV to RGB.
    /// Optimized version with reduced branching and fewer operations.
    pub fn from_hsv(h: f32, s: f32, v: f32) -> Color {
        let h = h % 360.0;
        let s = s.clamp(0.0, 1.0);
        let v = v.clamp(0.0, 1.0);

        let c = v * s;
        let h_sector = h / 60.0;
        let x = c * (1.0 - (h_sector % 2.0 - 1.0).abs());
        let m = v - c;

        // Use integer sector for faster branching
        let sector = h_sector as i32;
        let (r, g, b) = match sector {
            0 => (c, x, 0.0), // 0-60
            1 => (x, c, 0.0), // 60-120
            2 => (0.0, c, x), // 120-180
            3 => (0.0, x, c), // 180-240
            4 => (x, 0.0, c), // 240-300
            _ => (c, 0.0, x), // 300-360
        };

        // Single multiplication by 255.0 instead of per-component
        let scale = 255.0;
        Color {
            r: ((r + m) * scale) as u8,
            g: ((g + m) * scale) as u8,
            b: ((b + m) * scale) as u8,
        }
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::BLACK
    }
}

impl std::fmt::Display for Color {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
}

/// RGB lighting effect types.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub enum Effect {
    /// Static single color.
    Static { color: Color },

    /// Breathing effect (pulse).
    Breathing {
        color: Color,
        speed: f32, // Cycles per second
    },

    /// Color cycle through spectrum.
    Spectrum {
        speed: f32, // Cycles per second
    },

    /// Wave effect across zones.
    Wave {
        colors: Vec<Color>,
        speed: f32,
        direction: WaveDirection,
    },

    /// Reactive effect (responds to key presses).
    Reactive {
        color: Color,
        duration: f32, // Seconds
    },

    /// Gradient between two colors.
    Gradient { start: Color, end: Color },

    /// Custom per-zone colors.
    Custom { colors: Vec<Color> },

    /// Disabled (all LEDs off).
    Off,
}

/// Wave effect direction.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum WaveDirection {
    LeftToRight,
    RightToLeft,
    CenterOut,
    OutCenter,
}

impl Default for Effect {
    fn default() -> Self {
        Effect::Static { color: Color::WHITE }
    }
}

/// Timing mode for effect rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TimingMode {
    /// Fixed timing interval (legacy mode)
    Fixed,
    /// Adaptive timing based on effect complexity and system performance
    #[default]
    Adaptive,
    /// High performance mode with optimizations for consistent timing
    HighPerformance,
}

/// RGB effect engine that computes colors over time with adaptive timing support.
pub struct EffectEngine {
    effect: Effect,
    start_time: Instant,
    zone_count: usize,
    /// Cached colors from last computation
    cached_colors: Vec<Color>,
    /// Last time colors were computed
    last_compute_time: Duration,
    /// Timing mode for adaptive performance
    timing_mode: TimingMode,
    /// Dynamic cache threshold based on effect complexity
    cache_threshold: Duration,
    /// Base cache threshold (16ms = ~60 FPS)
    base_cache_threshold: Duration,
}

impl EffectEngine {
    /// Create a new effect engine with default adaptive timing.
    pub fn new(effect: Effect, zone_count: usize) -> Self {
        let mut engine = Self {
            effect,
            start_time: Instant::now(),
            zone_count,
            cached_colors: Vec::with_capacity(zone_count),
            last_compute_time: Duration::ZERO,
            timing_mode: TimingMode::default(),
            cache_threshold: Duration::from_millis(16), // Base threshold
            base_cache_threshold: Duration::from_millis(16),
        };

        // Calculate initial frame budget based on effect complexity
        engine.update_cache_threshold();
        engine
    }

    /// Create a new effect engine with specific timing mode.
    pub fn with_timing_mode(effect: Effect, zone_count: usize, timing_mode: TimingMode) -> Self {
        let mut engine = Self {
            effect,
            start_time: Instant::now(),
            zone_count,
            cached_colors: vec![Color::BLACK; zone_count],
            last_compute_time: Duration::ZERO,
            timing_mode,
            cache_threshold: Duration::from_millis(16),
            base_cache_threshold: Duration::from_millis(16),
        };

        engine.update_cache_threshold();
        engine
    }

    /// Set a new effect and update timing accordingly.
    pub fn set_effect(&mut self, effect: Effect) {
        self.effect = effect;
        self.start_time = Instant::now();
        self.last_compute_time = Duration::ZERO; // Force recompute on next call
        self.update_cache_threshold(); // Recalculate timing based on new effect
    }

    /// Set the timing mode.
    pub fn set_timing_mode(&mut self, mode: TimingMode) {
        self.timing_mode = mode;
        self.update_cache_threshold();
    }

    /// Get the current timing mode.
    pub fn timing_mode(&self) -> TimingMode {
        self.timing_mode
    }

    /// Calculate frame budget based on effect complexity and timing mode.
    pub fn calculate_frame_budget(&self) -> Duration {
        match self.timing_mode {
            TimingMode::Fixed => self.base_cache_threshold,
            TimingMode::Adaptive => {
                let complexity = self.calculate_effect_complexity();
                match complexity {
                    crate::performance::EffectComplexity::Simple => Duration::from_millis(33), // 30 FPS
                    crate::performance::EffectComplexity::Medium => Duration::from_millis(16), // 60 FPS
                    crate::performance::EffectComplexity::High => Duration::from_millis(8),    // 120 FPS
                    crate::performance::EffectComplexity::Critical => Duration::from_millis(4), // 240 FPS
                }
            }
            TimingMode::HighPerformance => {
                // Aggressive timing for consistent performance
                let complexity = self.calculate_effect_complexity();
                match complexity {
                    crate::performance::EffectComplexity::Simple => Duration::from_millis(12), // 83 FPS
                    crate::performance::EffectComplexity::Medium => Duration::from_millis(8),  // 120 FPS
                    crate::performance::EffectComplexity::High => Duration::from_millis(4),    // 240 FPS
                    crate::performance::EffectComplexity::Critical => Duration::from_millis(2), // 500 FPS
                }
            }
        }
    }

    /// Calculate effect complexity for the current effect.
    fn calculate_effect_complexity(&self) -> crate::performance::EffectComplexity {
        crate::performance::calculate_effect_complexity(&self.effect)
    }

    /// Update cache threshold based on current effect and timing mode.
    fn update_cache_threshold(&mut self) {
        self.cache_threshold = self.calculate_frame_budget();
    }

    /// Get the current effect.
    pub fn effect(&self) -> &Effect {
        &self.effect
    }

    /// Compute current colors for all zones.
    /// Uses caching to avoid unnecessary recomputation.
    #[inline]
    pub fn compute(&mut self) -> &[Color] {
        let elapsed = self.start_time.elapsed();

        // Return cached colors if delta is below threshold (but always compute on first call)
        if self.last_compute_time != Duration::ZERO
            && elapsed.saturating_sub(self.last_compute_time) < self.cache_threshold
        {
            return &self.cached_colors;
        }

        let elapsed_secs = elapsed.as_secs_f32();

        // Reuse cached vector to avoid allocations - use truncate instead of clear + resize
        self.cached_colors.clear();

        match &self.effect {
            Effect::Static { color } => {
                // Use extend with repeat iterator for better performance than resize
                self.cached_colors.extend(std::iter::repeat_n(*color, self.zone_count));
            }

            Effect::Breathing { color, speed } => {
                // Sine wave for breathing effect - use optimized sin approximation for better performance
                let t = (elapsed_secs * speed * 2.0 * std::f32::consts::PI).sin();
                let brightness = (t + 1.0) * 0.5; // Use multiplication instead of division
                let scaled_color = color.scale(brightness);
                self.cached_colors
                    .extend(std::iter::repeat_n(scaled_color, self.zone_count));
            }

            Effect::Spectrum { speed } => {
                // Cycle through hue
                let hue = (elapsed_secs * speed * 360.0) % 360.0;
                let color = Color::from_hsv(hue, 1.0, 1.0);
                self.cached_colors.extend(std::iter::repeat_n(color, self.zone_count));
            }

            Effect::Wave {
                colors,
                speed,
                direction,
            } => {
                if colors.is_empty() {
                    self.cached_colors
                        .extend(std::iter::repeat_n(Color::BLACK, self.zone_count));
                } else {
                    let phase = elapsed_secs * speed;
                    let zone_count_f32 = self.zone_count as f32;
                    let colors_len_f32 = colors.len() as f32;
                    let center = zone_count_f32 * 0.5; // Pre-calculate center

                    // Pre-allocate capacity to avoid reallocations
                    self.cached_colors.reserve(self.zone_count);

                    for i in 0..self.zone_count {
                        let i_f32 = i as f32;
                        let zone_offset = match direction {
                            WaveDirection::LeftToRight => i_f32 / zone_count_f32,
                            WaveDirection::RightToLeft => 1.0 - (i_f32 / zone_count_f32),
                            WaveDirection::CenterOut => (i_f32 - center).abs() / center,
                            WaveDirection::OutCenter => 1.0 - (i_f32 - center).abs() / center,
                        };

                        let t = (phase + zone_offset) % 1.0;
                        let color_pos = t * colors_len_f32;
                        let color_index = color_pos as usize % colors.len();
                        let next_index = (color_index + 1) % colors.len();
                        let blend_t = color_pos % 1.0;

                        // Safe indexing - indices are guaranteed valid by modulo operations
                        self.cached_colors
                            .push(Color::blend(colors[color_index], colors[next_index], blend_t));
                    }
                }
            }

            Effect::Reactive { color, .. } => {
                // Base state - actual reactivity handled by input events
                let scaled_color = color.scale(0.2);
                self.cached_colors
                    .extend(std::iter::repeat_n(scaled_color, self.zone_count));
            }

            Effect::Gradient { start, end } => {
                // Pre-allocate capacity and calculate division once
                self.cached_colors.reserve(self.zone_count);
                let divisor = ((self.zone_count - 1).max(1)) as f32;

                for i in 0..self.zone_count {
                    let t = i as f32 / divisor;
                    self.cached_colors.push(Color::blend(*start, *end, t));
                }
            }

            Effect::Custom { colors } => {
                let copy_len = colors.len().min(self.zone_count);
                self.cached_colors.extend_from_slice(&colors[..copy_len]);
                if self.zone_count > copy_len {
                    let remaining = self.zone_count - copy_len;
                    self.cached_colors.extend(std::iter::repeat_n(Color::BLACK, remaining));
                }
            }

            Effect::Off => {
                self.cached_colors
                    .extend(std::iter::repeat_n(Color::BLACK, self.zone_count));
            }
        }

        // Update last compute time
        self.last_compute_time = elapsed;

        &self.cached_colors
    }

    /// Reset the effect timer.
    pub fn reset(&mut self) {
        self.start_time = Instant::now();
    }
}

/// RGB controller for managing device lighting.
pub struct RgbController {
    engine: EffectEngine,
    brightness: f32,
    scaled_colors: Vec<Color>,
}

impl RgbController {
    /// Create a new RGB controller.
    pub fn new(zone_count: usize) -> Self {
        Self {
            engine: EffectEngine::new(Effect::default(), zone_count),
            brightness: 1.0,
            scaled_colors: Vec::with_capacity(zone_count),
        }
    }

    /// Set the lighting effect.
    pub fn set_effect(&mut self, effect: Effect) {
        self.engine.set_effect(effect);
    }

    /// Set brightness (0.0 to 1.0).
    pub fn set_brightness(&mut self, brightness: f32) {
        self.brightness = brightness.clamp(0.0, 1.0);
    }

    /// Get current brightness.
    pub fn brightness(&self) -> f32 {
        self.brightness
    }

    /// Get the current effect.
    pub fn effect(&self) -> &Effect {
        self.engine.effect()
    }

    /// Get the current timing mode.
    pub fn timing_mode(&self) -> TimingMode {
        self.engine.timing_mode()
    }

    /// Set the timing mode.
    pub fn set_timing_mode(&mut self, mode: TimingMode) {
        self.engine.set_timing_mode(mode);
    }

    /// Calculate frame budget for the current effect and timing mode.
    pub fn calculate_frame_budget(&self) -> Duration {
        self.engine.calculate_frame_budget()
    }

    /// Compute current colors with brightness applied.
    /// This method reuses an internal buffer to avoid allocations.
    #[inline]
    pub fn compute_colors(&mut self) -> &[Color] {
        let colors = self.engine.compute();

        // If brightness is 1.0, we can return the colors directly without scaling
        if (self.brightness - 1.0).abs() < f32::EPSILON {
            return colors;
        }

        // Apply brightness scaling reusing internal buffer
        self.scaled_colors.clear();
        self.scaled_colors
            .extend(colors.iter().map(|c| c.scale(self.brightness)));
        &self.scaled_colors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_parse_takes_names_and_hex() {
        assert_eq!(Color::parse("RED"), Some(Color::RED));
        assert_eq!(Color::parse("off"), Some(Color::BLACK));
        assert_eq!(Color::parse("#ff8000"), Some(Color::ORANGE));
        assert_eq!(Color::parse("ff8000"), Some(Color::ORANGE));
        assert_eq!(Color::parse("ff80"), None);
        assert_eq!(Color::parse("teal"), None);
    }

    #[test]
    fn test_color_from_hex() {
        assert_eq!(Color::from_hex(0xFF0000), Color::new(255, 0, 0));
        assert_eq!(Color::from_hex(0x00FF00), Color::new(0, 255, 0));
        assert_eq!(Color::from_hex(0x0000FF), Color::new(0, 0, 255));
        assert_eq!(Color::from_hex(0xFFFFFF), Color::WHITE);
        assert_eq!(Color::from_hex(0x000000), Color::BLACK);
    }

    #[test]
    fn test_color_to_hex() {
        assert_eq!(Color::RED.to_hex(), 0xFF0000);
        assert_eq!(Color::GREEN.to_hex(), 0x00FF00);
        assert_eq!(Color::BLUE.to_hex(), 0x0000FF);
        assert_eq!(Color::WHITE.to_hex(), 0xFFFFFF);
        assert_eq!(Color::BLACK.to_hex(), 0x000000);
    }

    #[test]
    fn test_color_blend() {
        let blended = Color::blend(Color::RED, Color::BLUE, 0.5);
        assert_eq!(blended.r, 127);
        assert_eq!(blended.g, 0);
        assert_eq!(blended.b, 127);

        assert_eq!(Color::blend(Color::RED, Color::BLUE, 0.0), Color::RED);
        assert_eq!(Color::blend(Color::RED, Color::BLUE, 1.0), Color::BLUE);
    }

    #[test]
    fn test_color_scale() {
        let color = Color::new(200, 150, 100);
        let scaled = color.scale(0.5);
        assert_eq!(scaled.r, 100);
        assert_eq!(scaled.g, 75);
        assert_eq!(scaled.b, 50);
        assert_eq!(color.scale(0.0), Color::BLACK);
        assert_eq!(color.scale(1.0), color);
    }

    #[test]
    fn test_color_from_hsv() {
        let red = Color::from_hsv(0.0, 1.0, 1.0);
        assert!(red.r > 250 && red.g < 5 && red.b < 5);

        let green = Color::from_hsv(120.0, 1.0, 1.0);
        assert!(green.r < 5 && green.g > 250 && green.b < 5);

        let blue = Color::from_hsv(240.0, 1.0, 1.0);
        assert!(blue.r < 5 && blue.g < 5 && blue.b > 250);

        let gray = Color::from_hsv(0.0, 0.0, 0.5);
        assert_eq!(gray.r, gray.g);
        assert_eq!(gray.g, gray.b);
    }

    #[test]
    fn test_effect_engine_static() {
        let effect = Effect::Static { color: Color::RED };
        let mut engine = EffectEngine::new(effect, 5);
        let colors = engine.compute();
        assert_eq!(colors.len(), 5);
        for color in colors {
            assert_eq!(*color, Color::RED);
        }
    }

    #[test]
    fn test_effect_engine_gradient() {
        let effect = Effect::Gradient {
            start: Color::RED,
            end: Color::BLUE,
        };
        let mut engine = EffectEngine::new(effect, 3);
        let colors = engine.compute();
        assert_eq!(colors.len(), 3);
        assert_eq!(colors[0], Color::RED);
        assert_eq!(colors[2], Color::BLUE);
        assert!(colors[1].r > 0 && colors[1].b > 0);
    }

    #[test]
    fn test_effect_engine_custom() {
        let custom_colors = vec![Color::RED, Color::GREEN, Color::BLUE];
        let effect = Effect::Custom { colors: custom_colors };
        let mut engine = EffectEngine::new(effect, 5);
        let colors = engine.compute();
        assert_eq!(colors.len(), 5);
        assert_eq!(colors[0], Color::RED);
        assert_eq!(colors[1], Color::GREEN);
        assert_eq!(colors[2], Color::BLUE);
        assert_eq!(colors[3], Color::BLACK);
        assert_eq!(colors[4], Color::BLACK);
    }

    #[test]
    fn test_effect_engine_off() {
        let effect = Effect::Off;
        let mut engine = EffectEngine::new(effect, 5);
        let colors = engine.compute();
        assert_eq!(colors.len(), 5);
        for color in colors {
            assert_eq!(*color, Color::BLACK);
        }
    }

    #[test]
    fn test_rgb_controller_brightness() {
        let mut controller = RgbController::new(3);
        controller.set_effect(Effect::Static {
            color: Color::new(200, 200, 200),
        });

        controller.set_brightness(1.0);
        let colors = controller.compute_colors();
        assert!(colors[0].r > 195 && colors[0].r <= 200);

        controller.set_brightness(0.5);
        let colors = controller.compute_colors();
        assert!(colors[0].r >= 95 && colors[0].r <= 105);

        controller.set_brightness(0.0);
        let colors = controller.compute_colors();
        assert_eq!(colors[0], Color::BLACK);
    }

    #[test]
    fn test_effect_engine_caching() {
        let effect = Effect::Static { color: Color::RED };
        let mut engine = EffectEngine::new(effect, 5);
        let colors = engine.compute();
        assert_eq!(colors.len(), 5);
        let colors = engine.compute();
        assert_eq!(colors.len(), 5);
        for color in colors {
            assert_eq!(*color, Color::RED);
        }
    }
}
