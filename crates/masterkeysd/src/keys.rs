//! Key press events for the GUI's live keyboard: read-only evdev, only while someone is subscribed.

use crate::engine::Recorder;
use crate::service::DaemonService;
use masterkeys_core::bindings::Macro;
use std::time::Instant;
use futures_util::StreamExt;
use masterkeys_core::dbus::OBJECT_PATH;
use std::collections::{BTreeSet, HashSet};
use std::sync::{Arc, Mutex};
use steelseries_gg::STEELSERIES_VENDOR_ID;
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};
use zbus::names::BusName;
use zbus::object_server::SignalEmitter;

pub const APEX3_PRODUCT: u16 = 0x161A;
/// How often to look for the keyboard's input nodes again after the reader ended.
const READER_RETRY: std::time::Duration = std::time::Duration::from_secs(1);

/// Merges key events from the board's interfaces (boot and NKRO both report keys) into one press/release
/// per real key transition.
#[derive(Default)]
pub struct PressTracker {
    held: HashSet<u16>,
}

impl PressTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed an evdev key event (`value` 1 down, 0 up, 2 autorepeat); returns a real transition, if any.
    pub fn feed(&mut self, code: u16, value: i32) -> Option<(u16, bool)> {
        match value {
            1 if self.held.insert(code) => Some((code, true)),
            0 if self.held.remove(&code) => Some((code, false)),
            _ => None,
        }
    }

    /// Releases for every held key (used when the reader stops).
    pub fn release_all(&mut self) -> Vec<(u16, bool)> {
        self.held.drain().map(|code| (code, false)).collect()
    }
}

/// Bus names of clients that asked for key events.
#[derive(Default, Debug)]
pub struct Subscribers {
    names: BTreeSet<String>,
}

impl Subscribers {
    /// Returns true if this is the first subscriber.
    pub fn add(&mut self, name: String) -> bool {
        let was_empty = self.names.is_empty();
        self.names.insert(name) && was_empty
    }

    /// Returns true if removing `name` left no subscribers.
    pub fn remove(&mut self, name: &str) -> bool {
        self.names.remove(name) && self.names.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.names.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// Read the Apex 3's key events (no grab) until `stop` turns true or a device fails, sending transitions.
/// Always ends by releasing any keys still held.
pub async fn read_keys(events: mpsc::Sender<(u16, bool)>, mut stop: watch::Receiver<bool>) {
    let (raw_tx, mut raw_rx) = mpsc::channel::<(u16, i32)>(256);
    let mut readers = Vec::new();
    for (path, device) in evdev::enumerate() {
        let id = device.input_id();
        if id.vendor() != STEELSERIES_VENDOR_ID || id.product() != APEX3_PRODUCT || device.supported_keys().is_none() {
            continue;
        }
        let mut stream = match device.into_event_stream() {
            Ok(stream) => stream,
            Err(e) => {
                warn!("cannot read keys from {}: {e}", path.display());
                continue;
            }
        };
        let raw_tx = raw_tx.clone();
        readers.push(tokio::spawn(async move {
            while let Ok(event) = stream.next_event().await {
                if event.event_type() == evdev::EventType::KEY && raw_tx.send((event.code(), event.value())).await.is_err() {
                    break;
                }
            }
        }));
    }
    drop(raw_tx);
    info!("key reader started ({} input devices)", readers.len());

    let mut tracker = PressTracker::new();
    loop {
        tokio::select! {
            raw = raw_rx.recv() => match raw {
                Some((code, value)) => {
                    if let Some(transition) = tracker.feed(code, value) {
                        let _ = events.send(transition).await;
                    }
                }
                None => break, // every device stream ended (unplugged)
            },
            _ = async { let _ = stop.wait_for(|stop| *stop).await; } => break,
        }
    }
    for reader in readers {
        reader.abort();
    }
    for transition in tracker.release_all() {
        let _ = events.send(transition).await;
    }
    info!("key reader stopped");
}

/// Name used for the single peer of a peer-to-peer connection, which has no bus names.
pub const P2P_PEER: &str = "p2p";

/// What needs the keys: subscribers, a macro recording, or bindings to apply.
#[derive(Default)]
struct HubState {
    subscribers: Subscribers,
    /// The recording, and whether it stopped taking keys (time limit reached).
    recording: Option<(Recorder, bool)>,
    bindings: bool,
}

impl HubState {
    fn wants_reader(&self) -> bool {
        !self.subscribers.is_empty() || self.recording.as_ref().is_some_and(|(_, frozen)| !frozen) || self.bindings
    }
}

/// Who needs key events, plus the switch that runs the key reader while anything does.
#[derive(Clone)]
pub struct KeyHub {
    state: Arc<Mutex<HubState>>,
    want_reader: Arc<watch::Sender<bool>>,
}

impl KeyHub {
    /// The hub and the receiver `run_reader` watches.
    pub fn new() -> (Self, watch::Receiver<bool>) {
        let (want_tx, want_rx) = watch::channel(false);
        let hub = Self { state: Arc::default(), want_reader: Arc::new(want_tx) };
        (hub, want_rx)
    }

    fn update(&self, change: impl FnOnce(&mut HubState)) {
        let mut state = self.state.lock().unwrap();
        change(&mut state);
        let want = state.wants_reader();
        self.want_reader.send_if_modified(|w| std::mem::replace(w, want) != want);
    }

    pub fn subscribe(&self, name: String) {
        info!("key subscriber added: {name}");
        self.update(|s| {
            s.subscribers.add(name);
        });
    }

    pub fn unsubscribe(&self, name: &str) {
        let mut known = false;
        self.update(|s| {
            known = s.subscribers.names().any(|n| n == name);
            s.subscribers.remove(name);
        });
        if known {
            info!("key subscriber removed: {name}");
        }
    }

    pub fn subscribers(&self) -> Vec<String> {
        self.state.lock().unwrap().subscribers.names().cloned().collect()
    }

    /// Whether the active profile has bindings (they need the reader running).
    pub fn set_bindings_active(&self, active: bool) {
        self.update(|s| s.bindings = active);
    }

    /// Start recording a macro, replacing any recording in progress; returns its start time.
    pub fn start_recording(&self) -> Instant {
        let now = Instant::now();
        self.update(|s| s.recording = Some((Recorder::start(now), false)));
        now
    }

    /// Stop taking keys into the recording started at `started`, if it is still the current one.
    pub fn freeze_recording(&self, started: Instant) {
        self.update(|s| {
            if let Some((recorder, frozen)) = &mut s.recording
                && recorder.started() == started
            {
                *frozen = true;
            }
        });
    }

    /// End the recording and return what it captured, or `None` if nothing was recording.
    pub fn stop_recording(&self) -> Option<Macro> {
        let mut taken = None;
        self.update(|s| taken = s.recording.take());
        taken.map(|(recorder, _)| recorder.finish())
    }

    /// A physical key transition, for the recording.
    pub fn record(&self, code: u16, pressed: bool, at: Instant) {
        if let Some((recorder, false)) = &mut self.state.lock().unwrap().recording {
            recorder.feed(code, pressed, at);
        }
    }
}

/// Send each key transition as `KeyEvent` to every subscriber, addressed to it alone.
pub async fn emit_keys(conn: zbus::Connection, hub: KeyHub, mut events: mpsc::Receiver<(u16, bool)>) {
    while let Some((code, pressed)) = events.recv().await {
        hub.record(code, pressed, Instant::now());
        for name in hub.subscribers() {
            let emitter = match SignalEmitter::new(&conn, OBJECT_PATH) {
                Ok(emitter) => emitter,
                Err(e) => {
                    warn!("cannot emit key events: {e}");
                    return;
                }
            };
            let emitter = if name == P2P_PEER {
                emitter
            } else {
                match BusName::try_from(name.as_str()) {
                    Ok(dest) => emitter.set_destination(dest),
                    Err(_) => continue,
                }
            };
            if let Err(e) = DaemonService::key_event(&emitter, code, pressed).await {
                warn!("cannot send key event to {name}: {e}");
            }
        }
    }
}

/// Run the key reader whenever `want` is true, stopping it when it turns false.
pub async fn run_reader(want: watch::Receiver<bool>, connected: watch::Receiver<bool>, events: mpsc::Sender<(u16, bool)>) {
    run_reader_with(want, connected, move |stop| read_keys(events.clone(), stop)).await
}

/// `run_reader` with the reader injected, for tests.
///
/// The reader ends by itself when the keyboard is unplugged (or was absent); while anyone is still
/// subscribed it is started again once the keyboard reconnects.
pub async fn run_reader_with<F, Fut>(mut want: watch::Receiver<bool>, connected: watch::Receiver<bool>, start: F)
where
    F: Fn(watch::Receiver<bool>) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    loop {
        if want.wait_for(|w| *w).await.is_err() {
            return;
        }
        let (stop_tx, stop_rx) = watch::channel(false);
        let mut reader = tokio::spawn(start(stop_rx));
        tokio::select! {
            _ = async { let _ = want.wait_for(|w| !*w).await; } => {
                stop_tx.send_replace(true);
                let _ = reader.await;
            }
            _ = &mut reader => {
                // The reader ended by itself (keyboard unplugged, or its input nodes not there yet). Check
                // again each second while the keyboard is connected; stop if nobody wants keys any more.
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(READER_RETRY) => {
                            if *connected.borrow() {
                                break;
                            }
                        }
                        _ = async { let _ = want.wait_for(|w| !*w).await; } => break,
                    }
                }
            }
        }
    }
}

/// Drop subscribers whose bus connection went away (e.g. the GUI crashed without unsubscribing).
pub async fn drop_vanished(conn: zbus::Connection, hub: KeyHub) -> zbus::Result<()> {
    let dbus = zbus::fdo::DBusProxy::new(&conn).await?;
    let mut changes = dbus.receive_name_owner_changed().await?;
    while let Some(change) = changes.next().await {
        let args = change.args()?;
        if args.new_owner().is_none() {
            hub.unsubscribe(args.name().as_str());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_interfaces_give_one_transition() {
        let mut t = PressTracker::new();
        assert_eq!(t.feed(30, 1), Some((30, true)));
        assert_eq!(t.feed(30, 1), None); // same press on the NKRO interface
        assert_eq!(t.feed(30, 2), None); // autorepeat
        assert_eq!(t.feed(30, 0), Some((30, false)));
        assert_eq!(t.feed(30, 0), None); // second interface's release
    }

    #[test]
    fn release_all_clears_held_keys() {
        let mut t = PressTracker::new();
        t.feed(30, 1);
        t.feed(31, 1);
        let mut ups = t.release_all();
        ups.sort();
        assert_eq!(ups, vec![(30, false), (31, false)]);
        assert_eq!(t.feed(30, 0), None);
    }

    #[test]
    fn subscribers_track_first_and_last() {
        let mut s = Subscribers::default();
        assert!(s.add(":1.5".into()));
        assert!(!s.add(":1.6".into()));
        assert!(!s.add(":1.5".into()));
        assert!(!s.remove(":1.5"));
        assert!(s.remove(":1.6"));
        assert!(!s.remove(":1.9"));
        assert!(s.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn reader_restarts_when_the_keyboard_reconnects() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let starts = Arc::new(AtomicUsize::new(0));
        let (want_tx, want_rx) = watch::channel(false);
        let (connected_tx, connected_rx) = watch::channel(true);
        let s = starts.clone();
        // A reader that ends at once, as when the keyboard is unplugged or absent.
        tokio::spawn(run_reader_with(want_rx, connected_rx, move |_stop| {
            s.fetch_add(1, Ordering::SeqCst);
            async {}
        }));
        let settle = || tokio::time::sleep(std::time::Duration::from_millis(50));

        want_tx.send_replace(true);
        settle().await;
        assert_eq!(starts.load(Ordering::SeqCst), 1);

        // Keyboard away: no restarts.
        connected_tx.send_replace(false);
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        assert_eq!(starts.load(Ordering::SeqCst), 1, "no restart while the keyboard is away");

        // Back: restarted within the 1 s re-check.
        connected_tx.send_replace(true);
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        let after_replug = starts.load(Ordering::SeqCst);
        assert!(after_replug >= 2, "restarted after replug");

        // Input nodes can appear a moment after the keyboard reconnects; a reader that finds none is retried.
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
        assert!(starts.load(Ordering::SeqCst) >= after_replug + 2, "re-checked while connected");

        // Nobody subscribed: no more readers.
        want_tx.send_replace(false);
        settle().await;
        let before = starts.load(Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        assert_eq!(starts.load(Ordering::SeqCst), before, "no reader without subscribers");
    }
}
