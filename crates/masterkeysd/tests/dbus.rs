//! The D-Bus interface, served over a private peer-to-peer connection (no session bus needed).

use futures_util::StreamExt;
use masterkeys_core::dbus::DaemonProxy;
use masterkeys_core::{LightingState, StateReport};
use masterkeys_core::profile::ProfileStore;
use masterkeysd::keys::{KeyHub, emit_keys};
use masterkeysd::persist::{load_profiles, saver};
use masterkeysd::service::{DaemonCore, DaemonService};
use masterkeys_core::devices::DeviceInfo;
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;
use std::sync::{Arc, Mutex};
use steelseries_gg::apex3::ZONE_COUNT;
use steelseries_gg::rgb::{Color, Effect};
use tokio::sync::{mpsc, watch};

struct Served {
    proxy: DaemonProxy<'static>,
    state: watch::Receiver<LightingState>,
    connected: watch::Sender<bool>,
    devices: watch::Sender<BTreeSet<String>>,
    frame: Arc<Mutex<Vec<Color>>>,
    keys: mpsc::Sender<(u16, bool)>,
    want_reader: watch::Receiver<bool>,
    bindings: watch::Receiver<masterkeys_core::bindings::Compiled>,
    core: DaemonCore,
    server: zbus::Connection,
    _config: Option<tempfile::TempDir>,
}

async fn serve() -> Served {
    let dir = tempfile::tempdir().unwrap();
    let mut s = serve_in(dir.path()).await;
    s._config = Some(dir);
    s
}

/// Serve a daemon whose profiles live in `config` (with the saver running).
async fn serve_in(config: &Path) -> Served {
    let store = ProfileStore::new(config);
    let (profiles, _warnings) = load_profiles(&store);
    let (state_tx, state_rx) = watch::channel(profiles.active().lighting.clone());
    let (connected_tx, connected_rx) = watch::channel(false);
    let (devices_tx, devices_rx) = watch::channel(BTreeSet::new());
    let frame = Arc::new(Mutex::new(vec![Color::WHITE; ZONE_COUNT]));
    let (hub, want_reader) = KeyHub::new();
    let (keys_tx, keys_rx) = mpsc::channel(16);
    let (dirty_tx, dirty_rx) = watch::channel(1u64);
    let profiles = Arc::new(Mutex::new(profiles));
    tokio::spawn(saver(store, profiles.clone(), dirty_rx));
    let core = DaemonCore {
        state: Arc::new(state_tx),
        connected: connected_rx,
        devices: devices_rx,
        frame: frame.clone(),
        keys: hub.clone(),
        profiles,
        dirty: Arc::new(dirty_tx),
        bindings: Arc::new(watch::channel(Default::default()).0),
        input: Arc::new(watch::channel(Default::default()).0),
        gamesense: Arc::new(watch::channel(Default::default()).0),
    };
    core.gamesense.send_modify(|g| g.enabled = core.profiles.lock().unwrap().gamesense_enabled());
    core.refresh_bindings();
    let bindings = core.bindings.subscribe();
    let core_handle = core.clone();
    let service = DaemonService::new(core.clone());
    let (server, client) = masterkeysd::p2p::pair(service).await.unwrap();
    tokio::spawn(emit_keys(server.clone(), hub, keys_rx));
    tokio::spawn(masterkeysd::service::watch_devices(server.clone(), core.clone()));
    let proxy = DaemonProxy::new(&client).await.unwrap();
    Served { proxy, state: state_rx, connected: connected_tx, devices: devices_tx, frame, keys: keys_tx, want_reader, bindings, core: core_handle, server, _config: None }
}

async fn report(s: &Served) -> StateReport {
    serde_json::from_str(&s.proxy.get_state().await.unwrap()).unwrap()
}

#[tokio::test]
async fn set_all_color_updates_state() {
    let s = serve().await;
    s.proxy.set_all_color("ff0000").await.unwrap();
    assert_eq!(report(&s).await.state.effect, Effect::Static { color: Color::RED });
    assert_eq!(s.state.borrow().effect, Effect::Static { color: Color::RED });
}

#[tokio::test]
async fn set_zone_color_during_animation_freezes_the_shown_frame() {
    let s = serve().await;
    s.proxy.set_effect(r#"{"Spectrum":{"speed":1.0}}"#).await.unwrap();
    *s.frame.lock().unwrap() = vec![Color::BLUE; ZONE_COUNT];
    s.proxy.set_zone_color(3, "00ff00").await.unwrap();
    let mut want = vec![Color::BLUE; ZONE_COUNT];
    want[3] = Color::GREEN;
    assert_eq!(s.state.borrow().effect, Effect::Custom { colors: want.clone() });
    // a second zone edits the existing Custom, not the frame
    *s.frame.lock().unwrap() = vec![Color::BLACK; ZONE_COUNT];
    s.proxy.set_zone_color(0, "ff0000").await.unwrap();
    want[0] = Color::RED;
    assert_eq!(s.state.borrow().effect, Effect::Custom { colors: want });
}

#[tokio::test]
async fn invalid_input_is_rejected_and_state_unchanged() {
    let s = serve().await;
    let three = r#"{"Custom":{"colors":[{"r":1,"g":2,"b":3},{"r":1,"g":2,"b":3},{"r":1,"g":2,"b":3}]}}"#;
    let reactive = r#"{"Reactive":{"color":{"r":255,"g":0,"b":0},"duration":1.0}}"#;
    assert!(s.proxy.set_zone_color(12, "ff0000").await.is_err());
    assert!(s.proxy.set_all_color("zz0000").await.is_err());
    assert!(s.proxy.set_brightness(150).await.is_err());
    assert!(s.proxy.set_effect("{not json").await.is_err());
    assert!(s.proxy.set_effect(three).await.is_err());
    let err = s.proxy.set_effect(reactive).await.unwrap_err().to_string();
    assert!(err.contains("Unsupported"), "{err}");
    assert_eq!(report(&s).await.state, LightingState::default());
}

#[tokio::test]
async fn set_effect_accepts_json() {
    let s = serve().await;
    s.proxy.set_effect(r#"{"Spectrum":{"speed":0.5}}"#).await.unwrap();
    assert_eq!(s.state.borrow().effect, Effect::Spectrum { speed: 0.5 });
}

#[tokio::test]
async fn state_changed_signal_fires() {
    let s = serve().await;
    let mut changes = s.proxy.receive_state_changed().await.unwrap();
    s.proxy.set_brightness(40).await.unwrap();
    let signal = changes.next().await.unwrap();
    let r: StateReport = serde_json::from_str(&signal.args().unwrap().state_json).unwrap();
    assert_eq!(r.state.brightness, 40);
}

#[tokio::test]
async fn get_state_reports_connection() {
    let s = serve().await;
    assert!(!report(&s).await.connected);
    s.connected.send_replace(true);
    assert!(report(&s).await.connected);
}

#[tokio::test]
async fn set_zone_color_right_after_a_colour_change_uses_the_new_colour() {
    // The output loop hasn't sent red yet, so the last frame is still black.
    let s = serve().await;
    *s.frame.lock().unwrap() = vec![Color::BLACK; ZONE_COUNT];
    s.proxy.set_all_color("ff0000").await.unwrap();
    s.proxy.set_zone_color(0, "00ff00").await.unwrap();
    let mut want = vec![Color::RED; ZONE_COUNT];
    want[0] = Color::GREEN;
    assert_eq!(s.state.borrow().effect, Effect::Custom { colors: want });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_zone_edits_are_all_kept() {
    let s = serve().await;
    for _ in 0..20 {
        s.proxy.set_all_color("000000").await.unwrap();
        let calls = (0..ZONE_COUNT as u8).map(|zone| s.proxy.set_zone_color(zone, "ff0000"));
        for result in futures_util::future::join_all(calls).await {
            result.unwrap();
        }
        assert_eq!(s.state.borrow().effect, Effect::Custom { colors: vec![Color::RED; ZONE_COUNT] });
    }
}

#[tokio::test]
async fn key_events_reach_subscribers_only_while_subscribed() {
    let s = serve().await;
    let mut events = s.proxy.receive_key_event().await.unwrap();
    assert!(!*s.want_reader.borrow(), "no reader before anyone subscribes");

    s.proxy.subscribe_keys().await.unwrap();
    assert!(*s.want_reader.borrow());
    s.keys.send((30, true)).await.unwrap();
    let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.next()).await.unwrap().unwrap();
    let args = event.args().unwrap();
    assert_eq!((args.code, args.pressed), (30, true));

    s.proxy.unsubscribe_keys().await.unwrap();
    assert!(!*s.want_reader.borrow(), "reader stops with the last subscriber");
    s.keys.send((31, true)).await.unwrap();
    let late = tokio::time::timeout(std::time::Duration::from_millis(200), events.next()).await;
    assert!(late.is_err(), "no key events after unsubscribing");
}

fn profile_file(config: &Path, name: &str) -> String {
    std::fs::read_to_string(config.join(format!("profiles/{name}.toml"))).unwrap_or_default()
}

async fn settle_saver() {
    tokio::time::sleep(Duration::from_millis(1600)).await;
}

#[tokio::test]
async fn edits_persist_to_the_active_profile_file() {
    let dir = tempfile::tempdir().unwrap();
    let s = serve_in(dir.path()).await;
    s.proxy.set_all_color("ff0000").await.unwrap();
    settle_saver().await;
    let file = profile_file(dir.path(), "Default");
    assert!(file.contains("Static") && file.contains("r = 255"), "{file}");
}

#[tokio::test]
async fn restart_restores_active_profile() {
    let dir = tempfile::tempdir().unwrap();
    {
        let s = serve_in(dir.path()).await;
        s.proxy.create_profile("Night").await.unwrap();
        s.proxy.activate_profile("Night").await.unwrap();
        s.proxy.set_brightness(20).await.unwrap();
        settle_saver().await;
    }
    let s = serve_in(dir.path()).await;
    let r = report(&s).await;
    assert_eq!((r.profile.as_str(), r.state.brightness), ("Night", 20));
    assert_eq!(s.state.borrow().brightness, 20, "output loop starts on the saved lighting");
}

#[tokio::test]
async fn profile_crud_over_dbus() {
    let s = serve().await;
    let mut changes = s.proxy.receive_profiles_changed().await.unwrap();
    let names = |json: String| -> Vec<String> {
        let list: Vec<masterkeys_core::profile::ProfileSummary> = serde_json::from_str(&json).unwrap();
        list.into_iter().map(|p| p.name).collect()
    };
    s.proxy.create_profile("A").await.unwrap();
    assert_eq!(names(s.proxy.list_profiles().await.unwrap()), ["A", "Default"]);
    s.proxy.rename_profile("A", "B").await.unwrap();
    s.proxy.set_profile_executables("B", &["game.exe"]).await.unwrap();
    s.proxy.delete_profile("B").await.unwrap();
    assert_eq!(names(s.proxy.list_profiles().await.unwrap()), ["Default"]);
    for _ in 0..4 {
        tokio::time::timeout(Duration::from_secs(2), changes.next()).await.expect("ProfilesChanged").unwrap();
    }
}

#[tokio::test]
async fn refusals() {
    let dir = tempfile::tempdir().unwrap();
    let s = serve_in(dir.path()).await;
    settle_saver().await;
    let before = profile_file(dir.path(), "Default");
    let err = s.proxy.delete_profile("Default").await.unwrap_err().to_string();
    assert!(err.contains("Refused"), "{err}");
    let err = s.proxy.rename_profile("Default", "../x").await.unwrap_err().to_string();
    assert!(err.contains("InvalidArgument"), "{err}");
    settle_saver().await;
    assert_eq!(profile_file(dir.path(), "Default"), before);
    assert!(!dir.path().join("x.toml").exists());
}

#[tokio::test]
async fn activate_switches_lighting() {
    let s = serve().await;
    s.proxy.set_all_color("ff0000").await.unwrap();
    s.proxy.create_profile("B").await.unwrap();
    s.proxy.activate_profile("B").await.unwrap();
    s.proxy.set_all_color("0000ff").await.unwrap();
    let mut states = s.proxy.receive_state_changed().await.unwrap();
    s.proxy.activate_profile("Default").await.unwrap();
    assert_eq!(s.state.borrow().effect, Effect::Static { color: Color::RED });
    let signal = tokio::time::timeout(Duration::from_secs(2), states.next()).await.unwrap().unwrap();
    let r: StateReport = serde_json::from_str(&signal.args().unwrap().state_json).unwrap();
    assert_eq!(r.profile, "Default");
}

#[tokio::test]
async fn remap_round_trip() {
    let s = serve().await;
    s.proxy.set_remap("caps_lock", "esc").await.unwrap();
    assert_eq!(report(&s).await.remaps.get("caps_lock").map(String::as_str), Some("esc"));
    assert_eq!(s.bindings.borrow().remaps.len(), 1);
    assert!(*s.want_reader.borrow(), "bindings turn the key reader on");
    s.proxy.clear_remap("caps_lock").await.unwrap();
    assert!(report(&s).await.remaps.is_empty());
    assert!(s.bindings.borrow().is_empty());
    assert!(!*s.want_reader.borrow());
}

#[tokio::test]
async fn macro_round_trip() {
    let s = serve().await;
    let json = r#"{"steps":[{"down":"a"},{"delay":20},{"up":"a"}]}"#;
    s.proxy.set_macro("f1", json).await.unwrap();
    assert_eq!(report(&s).await.macros["f1"].steps.len(), 3);
    assert_eq!(s.bindings.borrow().macros.len(), 1);
    let err = s.proxy.set_macro("f1", "{not json").await.unwrap_err();
    assert!(err.to_string().contains("InvalidArgument"), "{err}");
    s.proxy.clear_macro("f1").await.unwrap();
    assert!(report(&s).await.macros.is_empty());
}

#[tokio::test]
async fn bindings_follow_profile_switch() {
    let s = serve().await;
    s.proxy.set_remap("caps_lock", "esc").await.unwrap();
    s.proxy.create_profile("B").await.unwrap();
    assert!(report(&s).await.remaps.is_empty() || report(&s).await.profile == "Default");
    s.proxy.activate_profile("B").await.unwrap();
    assert!(s.bindings.borrow().is_empty(), "new profiles start with no bindings");
    s.proxy.activate_profile("Default").await.unwrap();
    assert_eq!(s.bindings.borrow().remaps.len(), 1);
}

#[tokio::test]
async fn unrelated_profile_edits_dont_touch_bindings() {
    let mut s = serve().await;
    s.proxy.set_remap("caps_lock", "esc").await.unwrap();
    s.bindings.mark_unchanged();
    s.proxy.create_profile("Other").await.unwrap();
    s.proxy.set_default_profile("Other").await.unwrap();
    s.proxy.set_profile_executables("Other", &["game"]).await.unwrap();
    s.proxy.set_brightness(40).await.unwrap();
    assert!(!s.bindings.has_changed().unwrap(), "same bindings must not re-notify (would undo an escape-chord stop)");
    s.proxy.set_remap("caps_lock", "tab").await.unwrap();
    assert!(s.bindings.has_changed().unwrap());
}

#[tokio::test]
async fn recording_captures_keys() {
    let s = serve().await;
    s.proxy.start_macro_recording().await.unwrap();
    assert!(*s.want_reader.borrow(), "recording turns the key reader on");
    s.keys.send((30, true)).await.unwrap();
    s.keys.send((30, false)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let m: masterkeys_core::bindings::Macro =
        serde_json::from_str(&s.proxy.stop_macro_recording().await.unwrap()).unwrap();
    use masterkeys_core::bindings::MacroStep;
    assert_eq!(m.steps.first(), Some(&MacroStep::Down("a".into())));
    assert_eq!(m.steps.last(), Some(&MacroStep::Up("a".into())));
    assert!(!*s.want_reader.borrow());
}

#[tokio::test]
async fn stop_without_start_is_invalid() {
    let s = serve().await;
    let err = s.proxy.stop_macro_recording().await.unwrap_err();
    assert!(err.to_string().contains("InvalidArgument"), "{err}");
    s.proxy.start_macro_recording().await.unwrap();
    let err = s.proxy.stop_macro_recording().await.unwrap_err();
    assert!(err.to_string().contains("InvalidArgument"), "nothing recorded: {err}");
}

#[tokio::test]
async fn bindings_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    {
        let s = serve_in(dir.path()).await;
        s.proxy.set_remap("caps_lock", "esc").await.unwrap();
        settle_saver().await;
    }
    let s = serve_in(dir.path()).await;
    assert_eq!(report(&s).await.remaps.get("caps_lock").map(String::as_str), Some("esc"));
    assert_eq!(s.bindings.borrow().remaps.len(), 1);
}

#[tokio::test]
async fn gamesense_toggle_persists() {
    let dir = tempfile::tempdir().unwrap();
    {
        let s = serve_in(dir.path()).await;
        assert!(report(&s).await.gamesense.enabled);
        s.proxy.set_game_sense_enabled(false).await.unwrap();
        assert!(!report(&s).await.gamesense.enabled);
        settle_saver().await;
    }
    let s = serve_in(dir.path()).await;
    assert!(!report(&s).await.gamesense.enabled);
}

#[tokio::test]
async fn gamesense_reports_active_game() {
    use steelseries_gg::gamesense::GameSenseOutput;
    let s = serve().await;
    let (tx, rx) = mpsc::unbounded_channel();
    let (overlay_tx, mut overlay_rx) = watch::channel([None; ZONE_COUNT]);
    tokio::spawn(masterkeysd::gamesense::follow(s.core.clone(), s.server.clone(), rx, overlay_tx));
    tx.send(GameSenseOutput::Color { game: "G".into(), zone: "all".into(), color: Color::RED }).unwrap();
    overlay_rx.wait_for(|o| o.iter().all(Option::is_some)).await.unwrap();
    assert_eq!(report(&s).await.gamesense.game.as_deref(), Some("G"));
    s.proxy.set_game_sense_enabled(false).await.unwrap();
    overlay_rx.wait_for(|o| o.iter().all(Option::is_none)).await.unwrap();
    tx.send(GameSenseOutput::Color { game: "G".into(), zone: "one".into(), color: Color::RED }).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(overlay_rx.borrow().iter().all(Option::is_none), "ignored while disabled");
    assert_eq!(report(&s).await.gamesense.game, None);
}

fn plug(s: &Served, ids: &[&str]) {
    s.devices.send_replace(ids.iter().map(|i| i.to_string()).collect());
}

#[tokio::test]
async fn devices_lists_connected_infos() {
    let s = serve().await;
    assert!(report(&s).await.devices.is_empty());
    plug(&s, &["k745pro", "apex3"]);
    assert_eq!(report(&s).await.devices, vec!["apex3".to_string(), "k745pro".to_string()]);
    let infos: Vec<DeviceInfo> = serde_json::from_str(&s.proxy.devices().await.unwrap()).unwrap();
    assert_eq!(infos, vec![DeviceInfo::apex3(), DeviceInfo::k745pro()]);
}

#[tokio::test]
async fn state_changed_on_device_change() {
    let s = serve().await;
    let mut changes = s.proxy.receive_state_changed().await.unwrap();
    plug(&s, &["k745pro"]);
    let signal = tokio::time::timeout(Duration::from_secs(2), changes.next()).await.unwrap().unwrap();
    let r: StateReport = serde_json::from_str(&signal.args().unwrap().state_json).unwrap();
    assert_eq!(r.devices, vec!["k745pro".to_string()]);
}

#[tokio::test]
async fn led_colour_keeps_apex_colours() {
    let s = serve().await;
    s.proxy.set_effect(r#"{"Spectrum":{"speed":1.0}}"#).await.unwrap();
    let shown: Vec<Color> = (0..10).map(|i| Color::new(i * 10, 1, 2)).collect();
    *s.frame.lock().unwrap() = shown.clone();
    s.proxy.set_led_color("k745pro", 0, "ff0000").await.unwrap();
    let state = s.state.borrow().clone();
    assert_eq!(state.effect, Effect::Custom { colors: shown });
    // The K745 keeps every other key as it looked; the coloured one changes.
    assert_eq!(state.custom["k745pro"].len(), DeviceInfo::k745pro().leds.len());
    assert_eq!(state.custom["k745pro"][&0], Color::RED);
    // On the Apex 3, an LED is a zone.
    s.proxy.set_led_color("apex3", 2, "0000ff").await.unwrap();
    let Effect::Custom { colors } = s.state.borrow().effect.clone() else { panic!("not custom") };
    assert_eq!(colors[2], Color::BLUE);
}

#[tokio::test]
async fn led_colour_rejects_unknown_device_and_led() {
    let s = serve().await;
    assert!(s.proxy.set_led_color("nope", 0, "ff0000").await.is_err());
    assert!(s.proxy.set_led_color("k745pro", 125, "ff0000").await.is_err(), "125 is not a K745 key LED");
    assert!(s.proxy.set_led_color("apex3", 10, "ff0000").await.is_err());
}

#[tokio::test]
async fn test_flash_restores_after_two_seconds() {
    let s = serve().await;
    s.proxy.set_all_color("00ff00").await.unwrap();
    s.proxy.test_flash("k745pro").await.unwrap();
    let flashing = s.state.borrow().clone();
    let leds = DeviceInfo::k745pro().leds;
    assert!(matches!(flashing.effect, Effect::Custom { .. }));
    assert_eq!(flashing.custom["k745pro"].len(), leds.len());
    assert!(flashing.custom["k745pro"].values().all(|c| *c == Color::RED));
    tokio::time::sleep(Duration::from_millis(2300)).await;
    assert_eq!(s.state.borrow().effect, Effect::Static { color: Color::GREEN });
}

#[tokio::test]
async fn test_flash_is_not_saved() {
    let s = serve().await;
    s.proxy.set_all_color("00ff00").await.unwrap();
    s.proxy.test_flash("apex3").await.unwrap();
    let profile = s.core.profiles.lock().unwrap().active().lighting.clone();
    assert_eq!(profile.effect, Effect::Static { color: Color::GREEN });
    assert!(s.proxy.test_flash("nope").await.is_err());
}

#[tokio::test]
async fn edit_during_flash_does_not_save_red() {
    let s = serve().await;
    s.proxy.set_all_color("00ff00").await.unwrap();
    s.proxy.test_flash("k745pro").await.unwrap();
    s.proxy.set_brightness(50).await.unwrap();
    let saved = s.core.profiles.lock().unwrap().active().lighting.clone();
    assert_eq!(saved.effect, Effect::Static { color: Color::GREEN });
    assert!(saved.custom.is_empty(), "{:?}", saved.custom);
    assert_eq!(saved.brightness, 50);
    tokio::time::sleep(Duration::from_millis(2300)).await;
    assert_eq!(s.state.borrow().effect, Effect::Static { color: Color::GREEN });
}

#[tokio::test]
async fn led_colour_keeps_other_k745_keys() {
    let s = serve().await;
    s.proxy.set_all_color("ffffff").await.unwrap();
    s.proxy.set_led_color("k745pro", 0, "ff0000").await.unwrap();
    let state = s.state.borrow().clone();
    let map = &state.custom["k745pro"];
    let leds = DeviceInfo::k745pro().leds;
    assert_eq!(map.len(), leds.len());
    assert_eq!(map[&0], Color::RED);
    assert!(leds.iter().filter(|l| l.index != 0).all(|l| map[&l.index] == Color::WHITE));
}
