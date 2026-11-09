//! The GUI's daemon client against a real DaemonService served over a peer-to-peer connection.

use masterkeys::client::{Command, Event, Status, run_on};
use masterkeys_core::LightingState;
use masterkeys_core::dbus::OBJECT_PATH;
use masterkeysd::keys::{KeyHub, emit_keys};
use masterkeys_core::profile::ProfileStore;
use masterkeysd::persist::load_profiles;
use masterkeysd::service::{DaemonCore, DaemonService};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use steelseries_gg::apex3::ZONE_COUNT;
use steelseries_gg::rgb::{Color, Effect};
use tokio::sync::{mpsc, watch};
use zbus::{Guid, connection::Builder};

struct Harness {
    events: mpsc::UnboundedReceiver<Event>,
    commands: mpsc::UnboundedSender<Command>,
    state: watch::Receiver<LightingState>,
    connected: watch::Sender<bool>,
    keys: mpsc::Sender<(u16, bool)>,
    server: zbus::Connection,
    _config: tempfile::TempDir,
}

async fn start() -> Harness {
    let (state_tx, state_rx) = watch::channel(LightingState::default());
    let (connected_tx, connected_rx) = watch::channel(false);
    let frame = Arc::new(Mutex::new([Color::WHITE; ZONE_COUNT]));
    let (hub, _want) = KeyHub::new();
    let (keys_tx, keys_rx) = mpsc::channel(16);
    let config = tempfile::tempdir().unwrap();
    let (profiles, _) = load_profiles(&ProfileStore::new(config.path()));
    let (dirty_tx, _dirty_rx) = watch::channel(0u64);
    let core = DaemonCore {
        state: Arc::new(state_tx),
        connected: connected_rx,
        frame,
        keys: hub.clone(),
        profiles: Arc::new(Mutex::new(profiles)),
        dirty: Arc::new(dirty_tx),
        bindings: Arc::new(watch::channel(Default::default()).0),
        input: Arc::new(watch::channel(Default::default()).0),
        gamesense: Arc::new(watch::channel(Default::default()).0),
    };
    core.gamesense.send_modify(|g| g.enabled = core.profiles.lock().unwrap().gamesense_enabled());
    core.refresh_bindings();
    let service = DaemonService::new(core);
    let (a, b) = UnixStream::pair().unwrap();
    let server = Builder::unix_stream(a).server(Guid::generate()).p2p().serve_at(OBJECT_PATH, service);
    let client = Builder::unix_stream(b).p2p();
    let (server, client) = tokio::join!(server.build(), client.build());
    let (server, client) = (server.unwrap(), client.unwrap());
    tokio::spawn(emit_keys(server.clone(), hub, keys_rx));

    let (event_tx, events) = mpsc::unbounded_channel();
    let (commands, mut command_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        run_on(client, &move |e| { let _ = event_tx.send(e); }, &mut command_rx, &mut true).await;
    });
    Harness { events, commands, state: state_rx, connected: connected_tx, keys: keys_tx, server, _config: config }
}

impl Harness {
    async fn next(&mut self) -> Event {
        tokio::time::timeout(Duration::from_secs(2), self.events.recv()).await.expect("event in time").unwrap()
    }

    /// Skip events until one matches.
    async fn until(&mut self, want: impl Fn(&Event) -> bool) -> Event {
        loop {
            let e = self.next().await;
            if want(&e) {
                return e;
            }
        }
    }
}

#[tokio::test]
async fn initial_state_and_status() {
    let mut h = start().await;
    assert!(matches!(h.next().await, Event::Status(Status::KeyboardNotFound)));
    assert!(matches!(h.next().await, Event::State(r) if r.state == LightingState::default()));
}

#[tokio::test]
async fn commands_reach_the_daemon() {
    let mut h = start().await;
    h.commands.send(Command::SetAll(Color::RED)).unwrap();
    h.until(|e| matches!(e, Event::State(r) if r.state.effect == Effect::Static { color: Color::RED })).await;
    assert_eq!(h.state.borrow().effect, Effect::Static { color: Color::RED });
}

#[tokio::test]
async fn device_connect_updates_status() {
    let mut h = start().await;
    h.next().await;
    h.next().await;
    h.connected.send_replace(true);
    // DeviceConnected is emitted by masterkeysd's watch_connection task; GetState also reports it
    h.commands.send(Command::SetBrightness(50)).unwrap();
    h.until(|e| matches!(e, Event::Status(Status::Connected))).await;
}

#[tokio::test]
async fn key_events_arrive() {
    let mut h = start().await;
    h.next().await;
    h.next().await;
    tokio::time::sleep(Duration::from_millis(100)).await; // let the client subscribe
    h.keys.send((30, true)).await.unwrap();
    h.until(|e| matches!(e, Event::Key(30, true))).await;
}

#[tokio::test]
async fn daemon_disappearing_reports_not_running_and_releases_keys() {
    let mut h = start().await;
    h.next().await;
    h.next().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.keys.send((30, true)).await.unwrap();
    h.until(|e| matches!(e, Event::Key(30, true))).await;
    h.server.clone().close().await.unwrap();
    h.until(|e| matches!(e, Event::Status(Status::DaemonNotRunning))).await;
    assert!(matches!(h.next().await, Event::Key(30, false)));
}

#[tokio::test]
async fn profiles_listed_on_connect() {
    let mut h = start().await;
    h.until(|e| matches!(e, Event::Profiles(list) if list.len() == 1 && list[0].name == "Default" && list[0].is_default))
        .await;
}

#[tokio::test]
async fn create_and_activate_profile() {
    let mut h = start().await;
    h.until(|e| matches!(e, Event::Profiles(_))).await;
    h.commands.send(Command::CreateProfile("A".into())).unwrap();
    h.until(|e| matches!(e, Event::Profiles(list) if list.iter().any(|p| p.name == "A"))).await;
    h.commands.send(Command::ActivateProfile("A".into())).unwrap();
    h.until(|e| matches!(e, Event::State(r) if r.profile == "A")).await;
}

#[tokio::test]
async fn remap_command_reaches_the_daemon() {
    let mut h = start().await;
    h.until(|e| matches!(e, Event::Profiles(_))).await;
    h.commands.send(Command::SetRemap("caps_lock".into(), "esc".into())).unwrap();
    h.until(|e| matches!(e, Event::State(r) if r.remaps.get("caps_lock").map(String::as_str) == Some("esc"))).await;
    h.commands.send(Command::ClearRemap("caps_lock".into())).unwrap();
    h.until(|e| matches!(e, Event::State(r) if r.remaps.is_empty())).await;
}

#[tokio::test]
async fn recording_puts_a_macro_on_the_key() {
    let mut h = start().await;
    h.until(|e| matches!(e, Event::Profiles(_))).await;
    h.commands.send(Command::StartRecording).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.keys.send((30, true)).await.unwrap();
    h.keys.send((30, false)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.commands.send(Command::StopRecording("f1".into())).unwrap();
    h.until(|e| matches!(e, Event::State(r) if r.macros.contains_key("f1"))).await;
}

#[tokio::test]
async fn keys_wanted_false_stops_key_events() {
    let mut h = start().await;
    h.until(|e| matches!(e, Event::Profiles(_))).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.commands.send(Command::KeysWanted(false)).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.keys.send((30, true)).await.unwrap();
    let got = tokio::time::timeout(Duration::from_millis(300), h.until(|e| matches!(e, Event::Key(30, true)))).await;
    assert!(got.is_err(), "no key events while not wanted");
    h.commands.send(Command::KeysWanted(true)).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.keys.send((31, true)).await.unwrap();
    h.until(|e| matches!(e, Event::Key(31, true))).await;
}

#[tokio::test]
async fn gamesense_toggle_reaches_daemon() {
    let mut h = start().await;
    h.until(|e| matches!(e, Event::Profiles(_))).await;
    h.commands.send(Command::SetGameSense(false)).unwrap();
    h.until(|e| matches!(e, Event::State(r) if !r.gamesense.enabled)).await;
}
