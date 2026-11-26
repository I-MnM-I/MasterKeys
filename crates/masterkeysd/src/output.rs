//! Owns the keyboard: connects, reconnects with backoff, and sends frames as the state changes.

use crate::device::{Lighting, Opener};
use masterkeys_core::LightingState;
use masterkeys_core::devices::DeviceInfo;
use masterkeys_core::render::Renderer;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use steelseries_gg::rgb::Color;
use tokio::sync::watch;
use tracing::{info, warn};

/// How often static lighting is re-sent.
pub const KEEPALIVE: Duration = Duration::from_secs(2);

/// Channels between one device's output loop and the rest of the daemon.
pub struct Shared {
    /// Latest wanted state; a watch keeps only the newest value, so bursts coalesce.
    pub state: watch::Receiver<LightingState>,
    /// Ids of the connected devices: each loop adds its own on connect and removes it when a write fails.
    pub devices: watch::Sender<BTreeSet<String>>,
    /// The frame last sent, for `SetZoneColor` (the Apex 3 only).
    pub frame: Option<Arc<Mutex<Vec<Color>>>>,
    /// GameSense colours laid over the frame (the Apex 3 only; zones that are `None` show the profile).
    pub overlay: Option<watch::Receiver<crate::gamesense::OverlayFrame>>,
}

/// Reconnect delays: start at `initial`, double after each failed open, stop at `max`.
#[derive(Clone, Copy, Debug)]
pub struct Backoff {
    pub initial: Duration,
    pub max: Duration,
}

impl Backoff {
    pub const DEFAULT: Backoff = Backoff { initial: Duration::from_secs(1), max: Duration::from_secs(10) };
    /// How often an absent device is looked for again.
    pub const SCAN: Backoff = Backoff { initial: Duration::from_secs(1), max: Duration::from_secs(3) };
}

/// Run forever: keep `device` showing the wanted state whenever it is plugged in.
pub async fn run(mut opener: Opener, device: DeviceInfo, mut shared: Shared, backoff: Backoff) {
    let mut renderer = Renderer::new(&device);
    loop {
        let mut lighting = connect(&mut opener, &device, backoff).await;
        shared.devices.send_modify(|d| {
            d.insert(device.id.clone());
        });
        info!("{} connected", device.name);
        let error = drive(lighting.as_mut(), &mut renderer, &mut shared).await;
        shared.devices.send_modify(|d| {
            d.remove(&device.id);
        });
        warn!("{} write failed, reconnecting: {error}", device.name);
        // Pause before reopening, so a device that opens but rejects writes can't spin this loop.
        tokio::time::sleep(backoff.initial).await;
    }
}

async fn connect(opener: &mut Opener, device: &DeviceInfo, backoff: Backoff) -> Box<dyn Lighting> {
    let mut delay = backoff.initial;
    let mut reported = false;
    loop {
        match opener() {
            Ok(lighting) => return lighting,
            Err(e) => {
                if !reported {
                    info!("waiting for {}: {e}", device.name);
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
    // False once GameSense went away (or for devices without it); then the overlay is never waited on.
    let mut overlay_alive = shared.overlay.is_some();
    let mut last_write: Option<tokio::time::Instant> = None;
    if let Err(e) = device.set_brightness(state.brightness) {
        return e;
    }
    loop {
        let frame = renderer.frame();
        let shown: Vec<Color> = match &mut shared.overlay {
            Some(overlay) => {
                let overlay = *overlay.borrow_and_update();
                frame.iter().enumerate().map(|(i, c)| overlay.get(i).copied().flatten().unwrap_or(*c)).collect()
            }
            None => frame.clone(),
        };
        // Wait out the device's minimum gap here, asleep, rather than blocking a runtime thread in the driver.
        if let Some(last) = last_write {
            tokio::time::sleep_until(last + device.min_gap()).await;
        }
        if let Err(e) = device.write(&shown) {
            return e;
        }
        last_write = Some(tokio::time::Instant::now());
        if let Some(sent) = &shared.frame {
            *sent.lock().unwrap() = frame;
        }

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
                renderer.set_state(&new);
                state = new;
            }
            changed = async {
                match (&mut shared.overlay, overlay_alive) {
                    (Some(overlay), true) => overlay.changed().await,
                    _ => std::future::pending().await,
                }
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
    use masterkeys_core::devices::{APEX3_ID, DeviceInfo, K745_ID};
    use std::collections::BTreeSet;
    use steelseries_gg::rgb::{Color, Effect};
    use tokio::sync::watch;

    #[derive(Clone, Debug, PartialEq)]
    enum Call {
        Zones(Vec<Color>),
        Brightness(u8),
    }

    /// Records calls; the write numbered `fail_at` (1-based, counted across all fakes) fails.
    struct Fake {
        calls: Arc<Mutex<Vec<Call>>>,
        writes: Arc<AtomicUsize>,
        fail_at: usize,
        info: masterkeys_core::devices::DeviceInfo,
    }

    impl Fake {
        fn record(&mut self, call: Call) -> steelseries_gg::Result<()> {
            if self.writes.fetch_add(1, Ordering::SeqCst) + 1 == self.fail_at {
                return Err(steelseries_gg::Error::DeviceCommunication("unplugged".into()));
            }
            self.calls.lock().unwrap().push(call);
            Ok(())
        }
    }

    impl Lighting for Fake {
        fn info(&self) -> &masterkeys_core::devices::DeviceInfo {
            &self.info
        }
        fn write(&mut self, c: &[Color]) -> steelseries_gg::Result<()> {
            self.record(Call::Zones(c.to_vec()))
        }
        fn set_brightness(&mut self, p: u8) -> steelseries_gg::Result<()> {
            self.record(Call::Brightness(p))
        }
    }

    struct Harness {
        calls: Arc<Mutex<Vec<Call>>>,
        opens: Arc<AtomicUsize>,
        state: watch::Sender<LightingState>,
        devices: watch::Receiver<BTreeSet<String>>,
        overlay: watch::Sender<crate::gamesense::OverlayFrame>,
        frame: Arc<Mutex<Vec<Color>>>,
    }

    impl Harness {
        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }
        fn count(&self, pred: impl Fn(&Call) -> bool) -> usize {
            self.calls().iter().filter(|c| pred(c)).count()
        }
        fn connected(&self) -> bool {
            self.devices.borrow().contains(APEX3_ID)
        }
    }

    /// Start the loop; opening fails `fail_opens` times first, and write number `fail_at` fails.
    fn start(fail_opens: usize, fail_at: usize) -> Harness {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let opens = Arc::new(AtomicUsize::new(0));
        let writes = Arc::new(AtomicUsize::new(0));
        let (state_tx, state_rx) = watch::channel(LightingState::default());
        let (devices_tx, devices_rx) = watch::channel(BTreeSet::new());
        let (c, o, w) = (calls.clone(), opens.clone(), writes.clone());
        let opener: Opener = Box::new(move || {
            if o.fetch_add(1, Ordering::SeqCst) < fail_opens {
                return Err(steelseries_gg::Error::DeviceNotFound("absent".into()));
            }
            Ok(Box::new(Fake { calls: c.clone(), writes: w.clone(), fail_at, info: masterkeys_core::devices::DeviceInfo::apex3() }) as Box<dyn Lighting>)
        });
        let (overlay_tx, overlay_rx) = watch::channel([None; ZONE_COUNT]);
        let frame = Arc::new(Mutex::new(vec![Color::BLACK; ZONE_COUNT]));
        let shared =
            Shared { state: state_rx, devices: devices_tx, frame: Some(frame.clone()), overlay: Some(overlay_rx) };
        tokio::spawn(run(opener, DeviceInfo::apex3(), shared, Backoff::DEFAULT));
        Harness { calls, opens, state: state_tx, devices: devices_rx, overlay: overlay_tx, frame }
    }

    /// A fake for `info` whose write number `fail_at` fails, recording into `calls`.
    fn fake_opener(info: DeviceInfo, calls: Arc<Mutex<Vec<Call>>>, fail_at: usize) -> Opener {
        let writes = Arc::new(AtomicUsize::new(0));
        Box::new(move || {
            Ok(Box::new(Fake { calls: calls.clone(), writes: writes.clone(), fail_at, info: info.clone() })
                as Box<dyn Lighting>)
        })
    }

    #[tokio::test(start_paused = true)]
    async fn one_device_failing_leaves_the_other_running() {
        let (state_tx, state_rx) = watch::channel(LightingState::default());
        let (devices_tx, mut devices) = watch::channel(BTreeSet::new());
        let (apex_calls, k745_calls) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(Vec::new())));
        let apex = Shared { state: state_rx.clone(), devices: devices_tx.clone(), frame: None, overlay: None };
        let k745 = Shared { state: state_rx, devices: devices_tx, frame: None, overlay: None };
        tokio::spawn(run(fake_opener(DeviceInfo::apex3(), apex_calls.clone(), 3), DeviceInfo::apex3(), apex, Backoff::SCAN));
        tokio::spawn(run(fake_opener(DeviceInfo::k745pro(), k745_calls.clone(), 0), DeviceInfo::k745pro(), k745, Backoff::SCAN));
        tokio::time::sleep(Duration::from_millis(50)).await;
        state_tx.send_replace(LightingState { effect: Effect::Static { color: Color::RED }, ..Default::default() });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let leds = DeviceInfo::k745pro().leds.len();
        assert_eq!(k745_calls.lock().unwrap().last(), Some(&Call::Zones(vec![Color::RED; leds])));
        assert!(devices.borrow_and_update().contains(K745_ID));
        // The Apex 3 fake failed its 3rd write; it is dropped, then found again by the next scan.
        tokio::time::sleep(Backoff::SCAN.max * 2).await;
        let now = devices.borrow_and_update().clone();
        assert!(now.contains(APEX3_ID) && now.contains(K745_ID), "{now:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn brightness_change_rewrites_the_frame() {
        let h = start(0, 0);
        tokio::time::sleep(Duration::from_millis(50)).await;
        h.state.send_replace(LightingState { brightness: 40, ..Default::default() });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let calls = h.calls();
        let b = calls.iter().rposition(|c| *c == Call::Brightness(40)).expect("brightness sent");
        assert!(calls[b + 1..].iter().any(|c| matches!(c, Call::Zones(_))), "{calls:?}");
    }

    #[tokio::test]
    async fn overlay_is_laid_over_frame() {
        let h = start(0, 0);
        h.state.send_replace(LightingState { effect: Effect::Static { color: Color::WHITE }, brightness: 100, ..Default::default() });
        wait(0.1).await;
        let mut overlay = [None; ZONE_COUNT];
        overlay[2] = Some(Color::RED);
        h.overlay.send_replace(overlay);
        wait(0.1).await;
        let mut want = [Color::WHITE; ZONE_COUNT];
        want[2] = Color::RED;
        assert_eq!(h.calls().last(), Some(&Call::Zones(want.to_vec())));
        assert_eq!(*h.frame.lock().unwrap(), vec![Color::WHITE; ZONE_COUNT], "frame keeps the profile's colours");
        h.overlay.send_replace([None; ZONE_COUNT]);
        wait(0.1).await;
        assert_eq!(h.calls().last(), Some(&Call::Zones(vec![Color::WHITE; ZONE_COUNT])));
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
        assert!(!h.connected());
        assert!(h.calls().is_empty());
        wait(5.0).await; // backoff 1s + 2s
        assert_eq!(h.opens.load(Ordering::SeqCst), 3);
        assert!(h.connected());
        // first brightness, then the colours (later writes are keepalives of the same frame)
        assert_eq!(h.calls()[..2], [Call::Brightness(100), Call::Zones(vec![Color::WHITE; ZONE_COUNT])]);
    }

    #[tokio::test(start_paused = true)]
    async fn write_error_drops_device_and_reconnects() {
        let h = start(0, 2); // the first Zones write fails
        wait(0.1).await;
        assert!(!h.connected(), "dropped after the failed write");
        wait(2.0).await;
        assert_eq!(h.opens.load(Ordering::SeqCst), 2);
        assert!(h.connected());
        // first connection got Brightness only; the reconnect re-applied the whole state
        assert_eq!(
            h.calls(),
            vec![Call::Brightness(100), Call::Brightness(100), Call::Zones(vec![Color::WHITE; ZONE_COUNT])]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn state_change_resends_frame_and_only_changed_brightness() {
        let h = start(0, 0);
        wait(0.1).await;
        h.state.send_replace(LightingState { effect: Effect::Static { color: Color::RED }, brightness: 100, ..Default::default() });
        wait(0.1).await;
        assert_eq!(h.calls().last(), Some(&Call::Zones(vec![Color::RED; ZONE_COUNT])));
        assert_eq!(h.count(|c| matches!(c, Call::Brightness(_))), 1);
        h.state.send_replace(LightingState { effect: Effect::Static { color: Color::RED }, brightness: 30, ..Default::default() });
        wait(0.1).await;
        assert!(h.calls().contains(&Call::Brightness(30)));
        assert_eq!(h.count(|c| matches!(c, Call::Brightness(_))), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn rapid_changes_coalesce_to_latest() {
        let h = start(0, 0);
        wait(0.1).await;
        for b in 51..=100u8 {
            h.state.send_replace(LightingState { effect: Effect::Off, brightness: b - 50, ..Default::default() });
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
        h.state.send_replace(LightingState { effect: Effect::Spectrum { speed: 1.0 }, brightness: 100, ..Default::default() });
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
        assert!(h.connected());
        wait(2.5).await;
        assert_eq!(h.opens.load(Ordering::SeqCst), 1, "not reopened yet (1 s pause)");
        assert!(!h.connected(), "unplug noticed by the keepalive");
    }

    /// Records the (virtual) time of every write and asks for a 35 ms gap.
    struct Gapped {
        info: DeviceInfo,
        at: Arc<Mutex<Vec<tokio::time::Instant>>>,
    }

    impl Lighting for Gapped {
        fn info(&self) -> &DeviceInfo {
            &self.info
        }
        fn write(&mut self, _: &[Color]) -> steelseries_gg::Result<()> {
            self.at.lock().unwrap().push(tokio::time::Instant::now());
            Ok(())
        }
        fn set_brightness(&mut self, _: u8) -> steelseries_gg::Result<()> {
            Ok(())
        }
        fn min_gap(&self) -> Duration {
            Duration::from_millis(35)
        }
    }

    #[tokio::test(start_paused = true)]
    async fn device_gap_is_waited_out_by_the_loop() {
        let at = Arc::new(Mutex::new(Vec::new()));
        let a = at.clone();
        let opener: Opener =
            Box::new(move || Ok(Box::new(Gapped { info: DeviceInfo::k745pro(), at: a.clone() }) as Box<dyn Lighting>));
        let spectrum = LightingState { effect: Effect::Spectrum { speed: 1.0 }, ..Default::default() };
        let (_state_tx, state_rx) = watch::channel(spectrum);
        let (devices_tx, _devices) = watch::channel(BTreeSet::new());
        let shared = Shared { state: state_rx, devices: devices_tx, frame: None, overlay: None };
        tokio::spawn(run(opener, DeviceInfo::k745pro(), shared, Backoff::SCAN));
        tokio::time::sleep(Duration::from_millis(500)).await;
        let at = at.lock().unwrap().clone();
        assert!(at.len() > 5, "{} writes", at.len());
        for w in at.windows(2) {
            assert!(w[1] - w[0] >= Duration::from_millis(35), "{:?}", w[1] - w[0]);
        }
    }
}
