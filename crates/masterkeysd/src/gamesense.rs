//! GameSense: game events colour zones over the profile's lighting until the game goes quiet.

use masterkeys_core::gszones::zones_for;
use masterkeys_core::layout::Layout;
use std::time::{Duration, Instant};
use steelseries_gg::apex3::ZONE_COUNT;
use steelseries_gg::gamesense::GameSenseOutput;
use steelseries_gg::rgb::Color;

/// A zone falls back to the profile this long after its game's last event or heartbeat.
pub const GAME_TIMEOUT: Duration = Duration::from_secs(15);

/// Per zone: the game colour shown over the profile, if any.
pub type OverlayFrame = [Option<Color>; ZONE_COUNT];

#[derive(Clone, Debug)]
struct Claim {
    color: Color,
    game: String,
    seen: Instant,
    /// Order of setting, for the active game.
    seq: u64,
}

/// Which zones games currently colour.
pub struct Overlay {
    layout: Layout,
    zones: [Option<Claim>; ZONE_COUNT],
    seq: u64,
}

impl Default for Overlay {
    fn default() -> Self {
        Self::new()
    }
}

impl Overlay {
    pub fn new() -> Self {
        Self { layout: Layout::apex3_ansi(), zones: Default::default(), seq: 0 }
    }

    /// Take one server output; true if the shown overlay changed.
    pub fn apply(&mut self, out: GameSenseOutput, now: Instant) -> bool {
        let before = self.frame();
        match out {
            GameSenseOutput::Color { game, zone, color } => {
                for z in zones_for(&zone, &self.layout) {
                    self.seq += 1;
                    self.zones[z] = Some(Claim { color, game: game.clone(), seen: now, seq: self.seq });
                }
            }
            GameSenseOutput::Heartbeat { game } => {
                for claim in self.zones.iter_mut().flatten().filter(|c| c.game == game) {
                    claim.seen = now;
                }
            }
            GameSenseOutput::GameRemoved { game } => {
                for slot in &mut self.zones {
                    if slot.as_ref().is_some_and(|c| c.game == game) {
                        *slot = None;
                    }
                }
            }
        }
        self.frame() != before
    }

    /// Drop zones whose game went quiet; true if any did.
    pub fn expire(&mut self, now: Instant) -> bool {
        let mut changed = false;
        for slot in &mut self.zones {
            if slot.as_ref().is_some_and(|c| now.saturating_duration_since(c.seen) >= GAME_TIMEOUT) {
                *slot = None;
                changed = true;
            }
        }
        changed
    }

    pub fn clear(&mut self) -> bool {
        let changed = self.zones.iter().any(Option::is_some);
        self.zones = Default::default();
        changed
    }

    pub fn frame(&self) -> OverlayFrame {
        std::array::from_fn(|i| self.zones[i].as_ref().map(|c| c.color))
    }

    /// The game that set a zone most recently.
    pub fn active_game(&self) -> Option<String> {
        self.zones.iter().flatten().max_by_key(|c| c.seq).map(|c| c.game.clone())
    }

    pub fn next_expiry(&self) -> Option<Instant> {
        self.zones.iter().flatten().map(|c| c.seen + GAME_TIMEOUT).min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use steelseries_gg::gamesense::GameSenseOutput;
    use steelseries_gg::rgb::Color;

    fn color(game: &str, zone: &str, c: Color) -> GameSenseOutput {
        GameSenseOutput::Color { game: game.into(), zone: zone.into(), color: c }
    }

    #[test]
    fn color_then_expire() {
        let mut o = Overlay::new();
        let t = Instant::now();
        assert!(o.apply(color("A", "all", Color::RED), t));
        assert_eq!(o.frame(), [Some(Color::RED); 10]);
        assert_eq!(o.next_expiry(), Some(t + GAME_TIMEOUT));
        assert!(!o.expire(t + Duration::from_secs(14)));
        assert!(o.expire(t + GAME_TIMEOUT));
        assert_eq!(o.frame(), [None; 10]);
        assert_eq!(o.active_game(), None);
    }

    #[test]
    fn heartbeat_extends() {
        let mut o = Overlay::new();
        let t = Instant::now();
        o.apply(color("A", "one", Color::RED), t);
        o.apply(GameSenseOutput::Heartbeat { game: "A".into() }, t + Duration::from_secs(10));
        assert!(!o.expire(t + Duration::from_secs(20)));
        assert_eq!(o.frame()[0], Some(Color::RED));
    }

    #[test]
    fn latest_wins_and_remove_clears_own_zones() {
        let mut o = Overlay::new();
        let t = Instant::now();
        o.apply(color("A", "all", Color::RED), t);
        o.apply(color("B", "one", Color::BLUE), t);
        assert_eq!(o.frame()[0], Some(Color::BLUE));
        assert_eq!(o.frame()[1], Some(Color::RED));
        assert_eq!(o.active_game().as_deref(), Some("B"));
        assert!(o.apply(GameSenseOutput::GameRemoved { game: "A".into() }, t));
        let f = o.frame();
        assert_eq!(f[0], Some(Color::BLUE));
        assert!(f[1..].iter().all(Option::is_none));
    }

    #[test]
    fn unknown_zone_changes_nothing() {
        let mut o = Overlay::new();
        assert!(!o.apply(color("A", "logo", Color::RED), Instant::now()));
        assert!(!o.clear());
    }
}
