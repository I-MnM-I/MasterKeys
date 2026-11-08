//! The input task: reads the Apex 3's keys for the GUI and recordings, and while the active profile
//! has bindings grabs the keyboard and replays it through a uinput virtual keyboard.

use crate::engine::Engine;
use crate::keys::{APEX3_PRODUCT, PressTracker, read_keys};
use crate::service::DaemonCore;
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, Device, EventType, InputEvent, KeyCode};
use masterkeys_core::InputStatus;
use masterkeys_core::bindings::Compiled;
use std::io;
use std::time::Duration;
use steelseries_gg::STEELSERIES_VENDOR_ID;
use steelseries_gg::input::{InputKey, KeyEvent, KeyValue};
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};

const EV_KEY: u16 = 1;
/// How long to wait for held keys (e.g. Enter that switched profile) to come up before grabbing.
const RELEASE_WAIT: Duration = Duration::from_secs(5);
/// Time for the desktop to notice the virtual keyboard before the first event.
const VIRTUAL_DEVICE_SETTLE: Duration = Duration::from_millis(300);
/// How often to look for the keyboard again after it went away.
const RETRY: Duration = Duration::from_secs(1);
/// Highest key code the virtual keyboard declares (KEY_MICMUTE).
const MAX_KEY_CODE: u16 = 248;
const CHORD_MESSAGE: &str = "remapping stopped by Ctrl+Ctrl+Shift+Shift; change a binding or switch profile to resume";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    Read,
    Grab,
}

/// What to do with the keys: grab while there are bindings to apply (unless stopped or impossible),
/// read while anything wants key events, otherwise nothing.
pub fn mode(want_read: bool, bindings_empty: bool, stopped: bool, uinput_ok: bool) -> Mode {
    if !bindings_empty && !stopped && uinput_ok {
        Mode::Grab
    } else if want_read || !bindings_empty {
        Mode::Read
    } else {
        Mode::Off
    }
}

/// True when no node reports a key held down.
pub fn all_released(states: &[Vec<u16>]) -> bool {
    states.iter().all(Vec::is_empty)
}

/// Where remapped key events go: the virtual keyboard, or a list in tests.
pub trait KeySink {
    fn emit(&mut self, events: &[KeyEvent]) -> io::Result<()>;
}

impl KeySink for Vec<KeyEvent> {
    fn emit(&mut self, events: &[KeyEvent]) -> io::Result<()> {
        self.extend_from_slice(events);
        Ok(())
    }
}

impl KeySink for VirtualDevice {
    fn emit(&mut self, events: &[KeyEvent]) -> io::Result<()> {
        for e in events {
            VirtualDevice::emit(self, &[InputEvent::new(EventType::KEY.0, e.key.code(), e.value.to_evdev())])?;
        }
        Ok(())
    }
}

/// An evdev event from one of the keyboard's nodes; `grabbed` nodes go through the engine.
#[derive(Clone, Copy, Debug)]
pub struct RawEvent {
    pub grabbed: bool,
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    /// Every node's stream ended (keyboard unplugged).
    Unplugged,
    /// The bindings became empty.
    Stopped,
    /// The escape chord fired.
    Chord,
    Error(String),
}

/// Run grabbed key events through the engine into the sink, and every key transition to the GUI,
/// until the keyboard goes away, the bindings empty, the chord fires or the sink fails. Always ends
/// by releasing every key the sink was told to press.
pub async fn pump(
    mut raw: mpsc::Receiver<RawEvent>,
    engine: &mut Engine,
    mut bindings: watch::Receiver<Compiled>,
    sink: &mut impl KeySink,
    gui: &mpsc::Sender<(u16, bool)>,
) -> Ended {
    let mut tracker = PressTracker::new();
    let ended = loop {
        let deadline = engine.next_deadline().map(tokio::time::Instant::from_std);
        let out = tokio::select! {
            event = raw.recv() => {
                let Some(event) = event else { break Ended::Unplugged };
                if event.kind != EV_KEY {
                    continue;
                }
                if let Some(transition) = tracker.feed(event.code, event.value) {
                    let _ = gui.send(transition).await;
                }
                let Some(value) = KeyValue::from_evdev(event.value) else { continue };
                if !event.grabbed {
                    continue;
                }
                engine.handle(KeyEvent { key: InputKey::from_code(event.code), value }, now())
            }
            _ = sleep_until(deadline) => engine.tick(now()),
            changed = bindings.changed() => {
                if changed.is_err() {
                    break Ended::Stopped;
                }
                let next = bindings.borrow_and_update().clone();
                if next.is_empty() {
                    break Ended::Stopped;
                }
                engine.replace(&next);
                continue;
            }
        };
        if let Err(e) = sink.emit(&out) {
            break Ended::Error(format!("virtual keyboard: {e}"));
        }
        if !engine.is_active() {
            break Ended::Chord;
        }
    };
    let release = engine.release_all();
    if let Err(e) = sink.emit(&release) {
        warn!("could not release virtual keys: {e}");
    }
    for (code, pressed) in tracker.release_all() {
        let _ = gui.send((code, pressed)).await;
    }
    ended
}

fn now() -> std::time::Instant {
    tokio::time::Instant::now().into_std()
}

async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
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
async fn grab_session(
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
        if all_released(&states) || waited.elapsed() >= RELEASE_WAIT {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
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

/// Run the keys as `mode` says, re-deciding whenever who wants keys, the bindings or the connection
/// change, and publishing what it does in `core.input`.
pub async fn run_input(core: DaemonCore, conn: zbus::Connection, mut want: watch::Receiver<bool>, gui: mpsc::Sender<(u16, bool)>) {
    let mut bindings = core.bindings.subscribe();
    let mut connected = core.connected.clone();
    let mut stopped = false;
    let mut unavailable: Option<String> = None;
    // A bindings change while not grabbed clears a chord stop or a failed grab, so they get retried.
    let mut rebound = false;
    loop {
        if std::mem::take(&mut rebound) {
            stopped = false;
            unavailable = None;
        }
        let empty = bindings.borrow().is_empty();
        let current = mode(*want.borrow(), empty, stopped, unavailable.is_none());
        let status = match current {
            Mode::Off => InputStatus::Off,
            Mode::Grab => InputStatus::Remapping,
            Mode::Read if stopped && !empty => InputStatus::Stopped(CHORD_MESSAGE.into()),
            Mode::Read => match &unavailable {
                Some(e) if !empty => InputStatus::Unavailable(e.clone()),
                _ => InputStatus::Reading,
            },
        };
        if core.input.send_if_modified(|s| std::mem::replace(s, status.clone()) != status) {
            core.announce(&conn, false).await;
        }

        let ended_by_itself = match current {
            Mode::Off => {
                tokio::select! {
                    r = want.changed() => if r.is_err() { return },
                    r = bindings.changed() => { if r.is_err() { return } rebound = true }
                }
                false
            }
            Mode::Read => {
                let (stop_tx, stop_rx) = watch::channel(false);
                let mut reader = tokio::spawn(read_keys(gui.clone(), stop_rx));
                tokio::select! {
                    _ = want.changed() => { stop_tx.send_replace(true); let _ = reader.await; false }
                    _ = bindings.changed() => { rebound = true; stop_tx.send_replace(true); let _ = reader.await; false }
                    _ = &mut reader => true,
                }
            }
            Mode::Grab => {
                let ended = grab_session(core.bindings.subscribe(), &gui).await;
                // Edits made while grabbed were applied by the engine; don't count them as a reset.
                bindings.mark_unchanged();
                match ended {
                    Ended::Unplugged => true,
                    Ended::Stopped => false,
                    Ended::Chord => {
                        warn!("{CHORD_MESSAGE}");
                        stopped = true;
                        false
                    }
                    Ended::Error(e) => {
                        warn!("remapping unavailable: {e}");
                        unavailable = Some(format!("{e} — install assets/70-masterkeys.rules (see the GUI)"));
                        false
                    }
                }
            }
        };
        if ended_by_itself {
            // The keyboard went away (or isn't there yet): look again each second while it is connected.
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(RETRY) => if *connected.borrow_and_update() { break },
                    r = want.changed() => { if r.is_err() { return } break }
                    r = bindings.changed() => { if r.is_err() { return } rebound = true; break }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use masterkeys_core::bindings::compile;
    use std::collections::BTreeMap;
    use steelseries_gg::input::{InputKey, KeyValue};

    fn compiled(remaps: &[(&str, &str)]) -> Compiled {
        let remaps: BTreeMap<String, String> = remaps.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        compile(&remaps, &BTreeMap::new()).0
    }
    fn code(name: &str) -> u16 {
        InputKey::parse(name).unwrap().code()
    }
    fn ev(name: &str, value: KeyValue) -> KeyEvent {
        KeyEvent { key: InputKey::parse(name).unwrap(), value }
    }
    fn key(name: &str, value: i32) -> RawEvent {
        RawEvent { grabbed: true, kind: EV_KEY, code: code(name), value }
    }

    struct Rig {
        raw: mpsc::Sender<RawEvent>,
        bindings: watch::Sender<Compiled>,
        gui: mpsc::Receiver<(u16, bool)>,
        task: tokio::task::JoinHandle<(Ended, Vec<KeyEvent>)>,
    }

    fn rig(remaps: &[(&str, &str)]) -> Rig {
        let (raw, raw_rx) = mpsc::channel(16);
        let (bindings, bindings_rx) = watch::channel(compiled(remaps));
        let (gui_tx, gui) = mpsc::channel(16);
        let task = tokio::spawn(async move {
            let mut engine = Engine::new(&bindings_rx.borrow());
            let mut sink: Vec<KeyEvent> = Vec::new();
            let ended = pump(raw_rx, &mut engine, bindings_rx, &mut sink, &gui_tx).await;
            (ended, sink)
        });
        Rig { raw, bindings, gui, task }
    }

    #[test]
    fn mode_table() {
        assert_eq!(mode(false, true, false, true), Mode::Off);
        assert_eq!(mode(true, true, false, true), Mode::Read);
        assert_eq!(mode(false, false, false, true), Mode::Grab);
        assert_eq!(mode(false, false, true, true), Mode::Read);
        assert_eq!(mode(false, false, false, false), Mode::Read);
    }

    #[test]
    fn all_released_checks_every_node() {
        assert!(all_released(&[vec![], vec![]]));
        assert!(!all_released(&[vec![], vec![30]]));
    }

    #[tokio::test]
    async fn pump_remaps_and_reports_physical_keys() {
        let mut r = rig(&[("caps_lock", "esc")]);
        r.raw.send(key("caps_lock", 1)).await.unwrap();
        assert_eq!(r.gui.recv().await, Some((code("caps_lock"), true)));
        r.raw.send(key("caps_lock", 0)).await.unwrap();
        assert_eq!(r.gui.recv().await, Some((code("caps_lock"), false)));
        drop(r.raw);
        let (ended, sink) = r.task.await.unwrap();
        assert_eq!(ended, Ended::Unplugged);
        assert_eq!(sink, vec![ev("esc", KeyValue::Press), ev("esc", KeyValue::Release)]);
    }

    #[tokio::test]
    async fn ungrabbed_nodes_only_go_to_the_gui() {
        let mut r = rig(&[("caps_lock", "esc")]);
        r.raw.send(RawEvent { grabbed: false, kind: EV_KEY, code: 115, value: 1 }).await.unwrap();
        assert_eq!(r.gui.recv().await, Some((115, true)));
        drop(r.raw);
        let (_, sink) = r.task.await.unwrap();
        assert!(sink.is_empty(), "{sink:?}");
    }

    #[tokio::test]
    async fn pump_releases_on_stream_end() {
        let r = rig(&[("caps_lock", "esc")]);
        r.raw.send(key("caps_lock", 1)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(r.raw);
        let (ended, sink) = r.task.await.unwrap();
        assert_eq!(ended, Ended::Unplugged);
        assert_eq!(sink, vec![ev("esc", KeyValue::Press), ev("esc", KeyValue::Release)]);
    }

    #[tokio::test]
    async fn pump_releases_when_bindings_go_empty() {
        let r = rig(&[("caps_lock", "esc")]);
        r.raw.send(key("caps_lock", 1)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        r.bindings.send_replace(Compiled::default());
        let (ended, sink) = r.task.await.unwrap();
        assert_eq!(ended, Ended::Stopped);
        assert_eq!(sink.last(), Some(&ev("esc", KeyValue::Release)));
    }

    #[tokio::test(start_paused = true)]
    async fn pump_reports_chord_stop() {
        let r = rig(&[("caps_lock", "esc")]);
        for k in ["left_ctrl", "right_ctrl", "left_shift", "right_shift"] {
            r.raw.send(key(k, 1)).await.unwrap();
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
        let (ended, sink) = r.task.await.unwrap();
        assert_eq!(ended, Ended::Chord);
        assert!(sink.contains(&ev("left_ctrl", KeyValue::Release)), "{sink:?}");
    }
}
