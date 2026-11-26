//! Linux input: grab the Apex 3's evdev nodes and replay them through a uinput virtual keyboard.

use super::{Ended, GrabDecision, KeySink, RawEvent, grab_decision, pump};
use crate::engine::Engine;
use crate::keys::{APEX3_PRODUCT, PressTracker};
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, Device, EventType, InputEvent, KeyCode};
use masterkeys_core::bindings::Compiled;
use std::io;
use std::time::Duration;
use steelseries_gg::STEELSERIES_VENDOR_ID;
use steelseries_gg::input::KeyEvent;
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};

/// Time for the desktop to notice the virtual keyboard before the first event.
const VIRTUAL_DEVICE_SETTLE: Duration = Duration::from_millis(300);
/// Highest key code the virtual keyboard declares (KEY_MICMUTE).
const MAX_KEY_CODE: u16 = 248;

impl KeySink for VirtualDevice {
    fn emit(&mut self, events: &[KeyEvent]) -> io::Result<()> {
        for e in events {
            VirtualDevice::emit(self, &[InputEvent::new(EventType::KEY.0, e.key.code(), e.value.to_evdev())])?;
        }
        Ok(())
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

/// The Apex 3's key nodes: (letter-key nodes to grab, other key nodes such as media keys).
fn key_nodes() -> (Vec<Device>, Vec<Device>) {
    let mut grab = Vec::new();
    let mut read = Vec::new();
    for (_, device) in evdev::enumerate() {
        let id = device.input_id();
        if id.vendor() != STEELSERIES_VENDOR_ID || id.product() != APEX3_PRODUCT {
            continue;
        }
        match device.supported_keys() {
            Some(keys) if keys.contains(KeyCode::KEY_A) => grab.push(device),
            Some(_) => read.push(device),
            None => {}
        }
    }
    (grab, read)
}

fn virtual_keyboard() -> io::Result<VirtualDevice> {
    let mut keys = AttributeSet::<KeyCode>::new();
    for code in 1..=MAX_KEY_CODE {
        keys.insert(KeyCode::new(code));
    }
    VirtualDevice::builder()?.name("MasterKeys virtual keyboard").with_keys(&keys)?.build()
}

/// Grab the keyboard and replay it through the virtual keyboard until `pump` ends.
pub async fn grab_session(
    bindings: watch::Receiver<Compiled>,
    gui: &mpsc::Sender<(u16, bool)>,
) -> Ended {
    let mut sink = match virtual_keyboard() {
        Ok(device) => device,
        Err(e) => return Ended::Error(format!("cannot create a virtual keyboard (/dev/uinput): {e}")),
    };
    let (mut targets, others) = key_nodes();
    if targets.is_empty() {
        return Ended::Unplugged;
    }
    // Wait for held keys to come up, so the desktop never misses their release.
    let waited = tokio::time::Instant::now();
    loop {
        let states: Vec<Vec<u16>> = targets
            .iter()
            .map(|d| d.get_key_state().map(|s| s.iter().map(|k| k.code()).collect()).unwrap_or_default())
            .collect();
        match grab_decision(&states, waited.elapsed()) {
            GrabDecision::Grab => break,
            GrabDecision::Wait => tokio::time::sleep(Duration::from_millis(50)).await,
            GrabDecision::GiveUp => return Ended::KeysHeld,
        }
    }
    for device in &mut targets {
        if let Err(e) = device.grab() {
            // Dropping the devices releases any grab already taken.
            return Ended::Error(format!("cannot grab the keyboard (is another program capturing it?): {e}"));
        }
    }
    info!("keyboard grabbed ({} nodes), remapping", targets.len());
    tokio::time::sleep(VIRTUAL_DEVICE_SETTLE).await;

    let (raw_tx, raw_rx) = mpsc::channel(256);
    let mut readers = Vec::new();
    for (device, grabbed) in targets.into_iter().map(|d| (d, true)).chain(others.into_iter().map(|d| (d, false))) {
        let mut stream = match device.into_event_stream() {
            Ok(stream) => stream,
            Err(e) => return Ended::Error(format!("cannot read the keyboard: {e}")),
        };
        let raw_tx = raw_tx.clone();
        readers.push(tokio::spawn(async move {
            while let Ok(e) = stream.next_event().await {
                let event = RawEvent { grabbed, kind: e.event_type().0, code: e.code(), value: e.value() };
                if raw_tx.send(event).await.is_err() {
                    break;
                }
            }
        }));
    }
    drop(raw_tx);
    let mut engine = Engine::new(&bindings.borrow());
    let ended = pump(raw_rx, &mut engine, bindings, &mut sink, gui).await;
    for reader in readers {
        reader.abort(); // closing the streams releases the grabs
    }
    info!("keyboard released ({ended:?})");
    ended
}

