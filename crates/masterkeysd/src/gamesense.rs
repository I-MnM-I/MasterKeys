//! GameSense: game events colour zones over the profile's lighting until the game goes quiet.

use crate::service::DaemonCore;
use masterkeys_core::gszones::zones_for;
use masterkeys_core::layout::Layout;
use std::time::{Duration, Instant};
use steelseries_gg::apex3::ZONE_COUNT;
use steelseries_gg::gamesense::{GameSenseOutput, GameSenseServer};
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};
use steelseries_gg::rgb::Color;

/// The port tried first, so coreProps.json stays valid across restarts.
pub const PORT: u16 = 51340;
/// How often coreProps.json is refreshed (new Proton prefixes appear when games first run).
const COREPROPS_INTERVAL: Duration = Duration::from_secs(30);

/// Serve GameSense on 127.0.0.1, publish its address, and colour zones from game events.
pub async fn run_gamesense(core: DaemonCore, conn: zbus::Connection, overlay_tx: watch::Sender<OverlayFrame>) {
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", PORT)).await {
        Ok(l) => l,
        Err(e) => {
            warn!("GameSense port {PORT} unavailable ({e}); using a random port");
            match tokio::net::TcpListener::bind(("127.0.0.1", 0)).await {
                Ok(l) => l,
                Err(e) => return warn!("GameSense disabled: cannot listen: {e}"),
            }
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    info!("GameSense server on 127.0.0.1:{port}");
    core.gamesense.send_modify(|g| g.port = port);
    let server = match GameSenseServer::new("127.0.0.1", port) {
        Ok(s) => s,
        Err(e) => return warn!("GameSense disabled: {e}"),
    };
    let (tx, rx) = mpsc::unbounded_channel();
    server.set_output(tx);
    tokio::spawn(async move {
        if let Err(e) = server.serve(listener).await {
            warn!("GameSense server stopped: {e}");
        }
    });
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    tokio::spawn(async move {
        loop {
            let warnings = match &home {
                Some(home) => crate::coreprops::write_all(home, port),
                None => crate::coreprops::write_targets(&[crate::coreprops::TMP_PATH.into()], port),
            };
            for w in warnings {
                warn!("coreProps.json: {w}");
            }
            tokio::time::sleep(COREPROPS_INTERVAL).await;
        }
    });
    follow(core, conn, rx, overlay_tx).await;
}

/// Feed server output into the overlay while GameSense is on, expire quiet games, and publish the
/// overlay and active game.
pub async fn follow(
    core: DaemonCore,
    conn: zbus::Connection,
    mut rx: mpsc::UnboundedReceiver<GameSenseOutput>,
    overlay_tx: watch::Sender<OverlayFrame>,
) {
    let mut overlay = Overlay::new();
    let mut status = core.gamesense.subscribe();
    loop {
        let expiry = overlay.next_expiry().map(tokio::time::Instant::from_std);
        let changed = tokio::select! {
            out = rx.recv() => {
                let Some(out) = out else { return };
                status.borrow().enabled && overlay.apply(out, std::time::Instant::now())
            }
            _ = async {
                match expiry {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            } => overlay.expire(std::time::Instant::now()),
            r = status.changed() => {
                if r.is_err() { return }
                let enabled = status.borrow_and_update().enabled;
                !enabled && overlay.clear()
            }
        };
        if !changed {
            continue;
        }
        overlay_tx.send_replace(overlay.frame());
        let game = overlay.active_game();
        if core.gamesense.send_if_modified(|g| std::mem::replace(&mut g.game, game.clone()) != game) {
            status.mark_unchanged();
            core.announce(&conn, false).await;
        }
    }
}

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
