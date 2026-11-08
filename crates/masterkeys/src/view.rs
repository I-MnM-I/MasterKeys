//! What each key in the drawing should look like.

use masterkeys_core::layout::Layout;
use steelseries_gg::apex3::ZONE_COUNT;
use steelseries_gg::rgb::Color;

/// Each key's preview colour, in layout order: its zone's colour scaled by brightness (0-100).
pub fn key_colors(layout: &Layout, frame: &[Color; ZONE_COUNT], brightness: u8) -> Vec<Color> {
    let scale = f32::from(brightness.min(100)) / 100.0;
    layout.keys.iter().map(|k| frame[usize::from(k.zone).min(ZONE_COUNT - 1)].scale(scale)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use masterkeys_core::layout::Layout;
    use steelseries_gg::apex3::ZONE_COUNT;
    use steelseries_gg::rgb::Color;

    #[test]
    fn keys_take_their_zone_colour_scaled_by_brightness() {
        let l = Layout::apex3_ansi();
        let mut frame = [Color::BLACK; ZONE_COUNT];
        frame[0] = Color::new(200, 100, 0);
        let c = key_colors(&l, &frame, 50);
        let esc = l.keys.iter().position(|k| k.code == 1).unwrap();
        assert_eq!(c[esc], Color::new(100, 50, 0)); // Esc is in zone 0
        assert_eq!(c.len(), l.keys.len());
    }
}
