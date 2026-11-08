//! Key press events for the GUI's live keyboard: read-only evdev, only while someone is subscribed.

use crate::service::DaemonService;
use futures_util::StreamExt;
use masterkeys_core::dbus::OBJECT_PATH;
use std::collections::{BTreeSet, HashSet};
use std::sync::{Arc, Mutex};
use steelseries_gg::STEELSERIES_VENDOR_ID;
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};
use zbus::names::BusName;
use zbus::object_server::SignalEmitter;

const APEX3_PRODUCT: u16 = 0x161A;

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

/// Who is subscribed, plus the switch that runs the key reader while anyone is.
#[derive(Clone)]
pub struct KeyHub {
    subscribers: Arc<Mutex<Subscribers>>,
    want_reader: Arc<watch::Sender<bool>>,
}

impl KeyHub {
    /// The hub and the receiver `run_reader` watches.
    pub fn new() -> (Self, watch::Receiver<bool>) {
        let (want_tx, want_rx) = watch::channel(false);
        let hub = Self { subscribers: Arc::default(), want_reader: Arc::new(want_tx) };
        (hub, want_rx)
    }

    pub fn subscribe(&self, name: String) {
        info!("key subscriber added: {name}");
        if self.subscribers.lock().unwrap().add(name) {
            self.want_reader.send_replace(true);
        }
    }

    pub fn unsubscribe(&self, name: &str) {
        let mut subscribers = self.subscribers.lock().unwrap();
        let known = subscribers.names().any(|n| n == name);
        if subscribers.remove(name) {
            self.want_reader.send_replace(false);
        }
        if known {
            info!("key subscriber removed: {name}");
        }
    }

    pub fn subscribers(&self) -> Vec<String> {
        self.subscribers.lock().unwrap().names().cloned().collect()
    }
}

/// Send each key transition as `KeyEvent` to every subscriber, addressed to it alone.
pub async fn emit_keys(conn: zbus::Connection, hub: KeyHub, mut events: mpsc::Receiver<(u16, bool)>) {
    while let Some((code, pressed)) = events.recv().await {
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
pub async fn run_reader(mut want: watch::Receiver<bool>, events: mpsc::Sender<(u16, bool)>) {
    loop {
        if want.wait_for(|w| *w).await.is_err() {
            return;
        }
        let (stop_tx, stop_rx) = watch::channel(false);
        let reader = tokio::spawn(read_keys(events.clone(), stop_rx));
        let _ = want.wait_for(|w| !*w).await;
        stop_tx.send_replace(true);
        let _ = reader.await;
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
}
