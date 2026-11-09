//! GameSense zone names to the Apex 3's ten lighting zones.

use crate::layout::Layout;
use steelseries_gg::apex3::ZONE_COUNT;

const NUMBERS: [&str; ZONE_COUNT] = ["one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten"];

/// Key codes in each named per-key zone.
fn group_codes(name: &str) -> Option<Vec<u16>> {
    Some(match name {
        "function-keys" => (59..=68).chain([87, 88]).collect(),
        "number-keys" => (2..=11).collect(),
        "arrow-keys" => vec![103, 105, 106, 108],
        "nav-cluster" => vec![102, 104, 107, 109, 110, 111],
        "keypad" => [55].into_iter().chain(71..=83).chain([96, 98]).collect(),
        _ => return None,
    })
}

/// The lighting zones (0 leftmost .. 9) a GameSense zone name covers; empty for zones the Apex 3
/// doesn't have (`logo`, `wheel`) and unknown names.
pub fn zones_for(name: &str, layout: &Layout) -> Vec<usize> {
    if let Some(i) = NUMBERS.iter().position(|n| *n == name) {
        return vec![i];
    }
    if name == "all" || name == "main-keyboard" {
        return (0..ZONE_COUNT).collect();
    }
    let Some(codes) = group_codes(name) else { return Vec::new() };
    let mut zones: Vec<usize> = layout
        .keys
        .iter()
        .filter(|k| codes.contains(&k.code))
        .map(|k| usize::from(k.zone).min(ZONE_COUNT - 1))
        .collect();
    zones.sort_unstable();
    zones.dedup();
    zones
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Layout;

    #[test]
    fn zone_names_map_to_bands() {
        let l = Layout::apex3_ansi();
        assert_eq!(zones_for("one", &l), vec![0]);
        assert_eq!(zones_for("ten", &l), vec![9]);
        assert_eq!(zones_for("all", &l), (0..10).collect::<Vec<_>>());
        assert_eq!(zones_for("main-keyboard", &l), (0..10).collect::<Vec<_>>());
        let keypad = zones_for("keypad", &l);
        assert!(!keypad.is_empty() && keypad.iter().all(|z| *z >= 8), "{keypad:?}");
        assert!(!zones_for("arrow-keys", &l).is_empty());
        assert!(!zones_for("function-keys", &l).is_empty());
        assert!(zones_for("logo", &l).is_empty());
        assert!(zones_for("bogus", &l).is_empty());
    }
}
