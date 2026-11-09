//! Owns the keyboard: connects, reconnects with backoff, and sends frames as the state changes.

use crate::device::{Lighting, Opener};
use masterkeys_core::render::Renderer;
use masterkeys_core::LightingState;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use steelseries_gg::apex3::ZONE_COUNT;
use steelseries_gg::rgb::Color;
use tokio::sync::watch;
use tracing::{info, warn};

/// How often static lighting is re-sent.
pub const KEEPALIVE: Duration = Duration::from_secs(2);

/// Channels between the output loop and the D-Bus service.
pub struct Shared {
    /// Latest wanted state; a watch keeps only the newest value, so bursts coalesce.
    pub state: watch::Receiver<LightingState>,
    pub connected: watch::Sender<bool>,
    /// The frame last sent, for `SetZoneColor`.
    pub frame: Arc<Mutex<[Color; ZONE_COUNT]>>,
    /// GameSense colours laid over the frame (zones that are `None` show the profile).
    pub overlay: watch::Receiver<crate::gamesense::OverlayFrame>,
}

/// Reconnect delays: start at `initial`, double after each failed open, stop at `max`.
#[derive(Clone, Copy, Debug)]
pub struct Backoff {
    pub initial: Duration,
    pub max: Duration,
}

impl Backoff {
    pub const DEFAULT: Backoff = Backoff { initial: Duration::from_secs(1), max: Duration::from_secs(10) };
}

/// Run forever: keep the keyboard showing the wanted state.
pub async fn run(mut opener: Opener, mut shared: Shared, backoff: Backoff) {
    let mut renderer = Renderer::new();
    loop {
        let mut device = connect(&mut opener, backoff).await;
        shared.connected.send_replace(true);
        info!("keyboard connected");
        let error = drive(device.as_mut(), &mut renderer, &mut shared).await;
        shared.connected.send_replace(false);
        warn!("keyboard write failed, reconnecting: {error}");
        // Pause before reopening, so a board that opens but rejects writes can't spin this loop.
        tokio::time::sleep(backoff.initial).await;
    }
}

async fn connect(opener: &mut Opener, backoff: Backoff) -> Box<dyn Lighting> {
    let mut delay = backoff.initial;
    let mut reported = false;
    loop {
        match opener() {
            Ok(device) => return device,
            Err(e) => {
                if !reported {
                    info!("waiting for keyboard: {e}");
                    reported = true;
                }
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(backoff.max);
            }
        }
    }
}

/// Send the state, then follow changes and animation ticks until a write fails.
async fn drive(device: &mut dyn Lighting, renderer: &mut Renderer, shared: &mut Shared) -> steelseries_gg::Error {
    let mut state = shared.state.borrow_and_update().clone();
    renderer.set_state(&state);
    // False once GameSense went away; then the overlay is never waited on again.
    let mut overlay_alive = true;
    if let Err(e) = device.set_brightness(state.brightness) {
        return e;
    }
    loop {
        let frame = renderer.frame();
        let overlay = *shared.overlay.borrow_and_update();
        let shown: [Color; ZONE_COUNT] = std::array::from_fn(|i| overlay[i].unwrap_or(frame[i]));
        if let Err(e) = device.set_zones(&shown) {
            return e;
        }
        *shared.frame.lock().unwrap() = frame;

        // Static lighting is re-sent as a keepalive: it notices an unplug (the write fails) and restores the
        // colours if the board reset itself.
        let next = renderer.next_frame_in().or(Some(KEEPALIVE));
        tokio::select! {
            changed = shared.state.changed() => {
                if changed.is_err() {
                    // The service is gone (shutting down): keep the last frame showing.
                    std::future::pending::<()>().await;
                }
                let new = shared.state.borrow_and_update().clone();
                if new.brightness != state.brightness
                    && let Err(e) = device.set_brightness(new.brightness)
                {
                    return e;
                }
                if new.effect != state.effect {
                    renderer.set_state(&new);
                }
                state = new;
            }
            changed = async {
                if overlay_alive { shared.overlay.changed().await } else { std::future::pending().await }
            } => {
                if changed.is_err() {
                    // GameSense is gone: keep following the profile, never wake for the overlay again.
                    overlay_alive = false;
                }
            }
            _ = sleep_for(next) => {}
        }
    }
}

async fn sleep_for(delay: Option<Duration>) {
    match delay {
        Some(d) => tokio::time::sleep(d).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Lighting, Opener};
    use masterkeys_core::LightingState;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use steelseries_gg::apex3::ZONE_COUNT;
    use steelseries_gg::rgb::{Color, Effect};
    use tokio::sync::watch;

    #[derive(Clone, Debug, PartialEq)]
    enum Call {
        Zones([Color; ZONE_COUNT]),
        Brightness(u8),
    }

    /// Records calls; the write numbered `fail_at` (1-based, counted across all fakes) fails.
    struct Fake {
        calls: Arc<Mutex<Vec<Call>>>,
        writes: Arc<AtomicUsize>,
        fail_at: usize,
    }

    impl Fake {
        fn write(&mut self, call: Call) -> steelseries_gg::Result<()> {
            if self.writes.fetch_add(1, Ordering::SeqCst) + 1 == self.fail_at {
                return Err(steelseries_gg::Error::DeviceCommunication("unplugged".into()));
            }
            self.calls.lock().unwrap().push(call);
            Ok(())
        }
    }

    impl Lighting for Fake {
        fn set_zones(&mut self, c: &[Color; ZONE_COUNT]) -> steelseries_gg::Result<()> {
            self.write(Call::Zones(*c))
        }
        fn set_brightness(&mut self, p: u8) -> steelseries_gg::Result<()> {
            self.write(Call::Brightness(p))
        }
    }

    struct Harness {
        calls: Arc<Mutex<Vec<Call>>>,
        opens: Arc<AtomicUsize>,
        state: watch::Sender<LightingState>,
        connected: watch::Receiver<bool>,
        overlay: watch::Sender<crate::gamesense::OverlayFrame>,
        frame: Arc<Mutex<[Color; ZONE_COUNT]>>,
    }

    impl Harness {
        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }
        fn count(&self, pred: impl Fn(&Call) -> bool) -> usize {
            self.calls().iter().filter(|c| pred(c)).count()
        }
    }

    /// Start the loop; opening fails `fail_opens` times first, and write number `fail_at` fails.
    fn start(fail_opens: usize, fail_at: usize) -> Harness {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let opens = Arc::new(AtomicUsize::new(0));
        let writes = Arc::new(AtomicUsize::new(0));
        let (state_tx, state_rx) = watch::channel(LightingState::default());
        let (connected_tx, connected_rx) = watch::channel(false);
        let (c, o, w) = (calls.clone(), opens.clone(), writes.clone());
        let opener: Opener = Box::new(move || {
            if o.fetch_add(1, Ordering::SeqCst) < fail_opens {
                return Err(steelseries_gg::Error::DeviceNotFound("absent".into()));
            }
            Ok(Box::new(Fake { calls: c.clone(), writes: w.clone(), fail_at }) as Box<dyn Lighting>)
        });
        let (overlay_tx, overlay_rx) = watch::channel([None; ZONE_COUNT]);
        let frame = Arc::new(Mutex::new([Color::BLACK; ZONE_COUNT]));
        let shared = Shared { state: state_rx, connected: connected_tx, frame: frame.clone(), overlay: overlay_rx };
        tokio::spawn(run(opener, shared, Backoff::DEFAULT));
        Harness { calls, opens, state: state_tx, connected: connected_rx, overlay: overlay_tx, frame }
    }

    #[tokio::test]
    async fn overlay_is_laid_over_frame() {
        let h = start(0, 0);
        h.state.send_replace(LightingState { effect: Effect::Static { color: Color::WHITE }, brightness: 100 });
        wait(0.1).await;
        let mut overlay = [None; ZONE_COUNT];
        overlay[2] = Some(Color::RED);
        h.overlay.send_replace(overlay);
        wait(0.1).await;
        let mut want = [Color::WHITE; ZONE_COUNT];
        want[2] = Color::RED;
        assert_eq!(h.calls().last(), Some(&Call::Zones(want)));
        assert_eq!(*h.frame.lock().unwrap(), [Color::WHITE; ZONE_COUNT], "frame keeps the profile's colours");
        h.overlay.send_replace([None; ZONE_COUNT]);
        wait(0.1).await;
        assert_eq!(h.calls().last(), Some(&Call::Zones([Color::WHITE; ZONE_COUNT])));
    }

    #[tokio::test]
    async fn losing_gamesense_does_not_spin() {
        let h = start(0, 0);
        wait(0.1).await;
        let Harness { calls, overlay, .. } = h;
        drop(overlay);
        let before = calls.lock().unwrap().len();
        wait(0.5).await;
        let after = calls.lock().unwrap().len();
        assert!(after - before <= 2, "{} writes in 0.5 s after the overlay sender went away", after - before);
    }

    async fn wait(secs: f32) {
        tokio::time::sleep(Duration::from_secs_f32(secs)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn absent_keyboard_then_connects_and_applies_state() {
        let h = start(2, 0);
        wait(0.5).await;
        assert!(!*h.connected.borrow());
        assert!(h.calls().is_empty());
        wait(5.0).await; // backoff 1s + 2s
        assert_eq!(h.opens.load(Ordering::SeqCst), 3);
        assert!(*h.connected.borrow());
        // first brightness, then the colours (later writes are keepalives of the same frame)
        assert_eq!(h.calls()[..2], [Call::Brightness(100), Call::Zones([Color::WHITE; ZONE_COUNT])]);
    }

    #[tokio::test(start_paused = true)]
    async fn write_error_drops_device_and_reconnects() {
        let h = start(0, 2); // the first Zones write fails
        wait(0.1).await;
        assert!(!*h.connected.borrow(), "dropped after the failed write");
        wait(2.0).await;
        assert_eq!(h.opens.load(Ordering::SeqCst), 2);
        assert!(*h.connected.borrow());
        // first connection got Brightness only; the reconnect re-applied the whole state
        assert_eq!(
            h.calls(),
            vec![Call::Brightness(100), Call::Brightness(100), Call::Zones([Color::WHITE; ZONE_COUNT])]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn state_change_resends_frame_and_only_changed_brightness() {
        let h = start(0, 0);
        wait(0.1).await;
        h.state.send_replace(LightingState { effect: Effect::Static { color: Color::RED }, brightness: 100 });
        wait(0.1).await;
        assert_eq!(h.calls().last(), Some(&Call::Zones([Color::RED; ZONE_COUNT])));
        assert_eq!(h.count(|c| matches!(c, Call::Brightness(_))), 1);
        h.state.send_replace(LightingState { effect: Effect::Static { color: Color::RED }, brightness: 30 });
        wait(0.1).await;
        assert!(h.calls().contains(&Call::Brightness(30)));
        assert_eq!(h.count(|c| matches!(c, Call::Brightness(_))), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn rapid_changes_coalesce_to_latest() {
        let h = start(0, 0);
        wait(0.1).await;
        for b in 51..=100u8 {
            h.state.send_replace(LightingState { effect: Effect::Off, brightness: b - 50 });
        }
        wait(0.1).await;
        let brightness: Vec<_> = h.calls().into_iter().filter_map(|c| match c {
            Call::Brightness(b) => Some(b),
            _ => None,
        }).collect();
        assert_eq!(brightness.last(), Some(&50));
        assert!(brightness.len() < 50, "{brightness:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn animation_sends_frames_on_a_timer() {
        let h = start(0, 0);
        wait(0.1).await;
        h.state.send_replace(LightingState { effect: Effect::Spectrum { speed: 1.0 }, brightness: 100 });
        wait(0.01).await;
        let before = h.count(|c| matches!(c, Call::Zones(_)));
        wait(1.0).await;
        let frames = h.count(|c| matches!(c, Call::Zones(_))) - before;
        // 16 ms cap: frames at 0, 16, ..., 992 ms is at most 63
        assert!((30..=63).contains(&frames), "{frames} frames in 1s");
    }

    #[tokio::test(start_paused = true)]
    async fn static_lighting_is_resent_as_a_keepalive() {
        let h = start(0, 0);
        wait(0.1).await;
        let before = h.count(|c| matches!(c, Call::Zones(_)));
        wait(5.0).await;
        let resent = h.count(|c| matches!(c, Call::Zones(_))) - before;
        assert!((2..=3).contains(&resent), "{resent} keepalive frames in 5 s");
    }

    #[tokio::test(start_paused = true)]
    async fn unplug_is_noticed_while_static() {
        let h = start(0, 3); // the first keepalive write fails, as after an unplug
        wait(0.1).await;
        assert!(*h.connected.borrow());
        wait(2.5).await;
        assert_eq!(h.opens.load(Ordering::SeqCst), 1, "not reopened yet (1 s pause)");
        assert!(!*h.connected.borrow(), "unplug noticed by the keepalive");
    }
}
