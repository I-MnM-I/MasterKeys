//! The D-Bus interface, served over a private peer-to-peer connection (no session bus needed).

use futures_util::StreamExt;
use masterkeys_core::dbus::{DaemonProxy, OBJECT_PATH};
use masterkeys_core::{LightingState, StateReport};
use masterkeysd::service::DaemonService;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use steelseries_gg::apex3::ZONE_COUNT;
use steelseries_gg::rgb::{Color, Effect};
use tokio::sync::watch;
use zbus::{Guid, connection::Builder};

struct Served {
    proxy: DaemonProxy<'static>,
    state: watch::Receiver<LightingState>,
    connected: watch::Sender<bool>,
    frame: Arc<Mutex<[Color; ZONE_COUNT]>>,
    _server: zbus::Connection,
}

async fn serve() -> Served {
    let (state_tx, state_rx) = watch::channel(LightingState::default());
    let (connected_tx, connected_rx) = watch::channel(false);
    let frame = Arc::new(Mutex::new([Color::WHITE; ZONE_COUNT]));
    let service = DaemonService::new(state_tx, connected_rx, frame.clone());
    let (a, b) = UnixStream::pair().unwrap();
    let server = Builder::unix_stream(a).server(Guid::generate()).p2p().serve_at(OBJECT_PATH, service);
    let client = Builder::unix_stream(b).p2p();
    let (server, client) = tokio::join!(server.build(), client.build());
    let (server, client) = (server.unwrap(), client.unwrap());
    let proxy = DaemonProxy::new(&client).await.unwrap();
    Served { proxy, state: state_rx, connected: connected_tx, frame, _server: server }
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
    *s.frame.lock().unwrap() = [Color::BLUE; ZONE_COUNT];
    s.proxy.set_zone_color(3, "00ff00").await.unwrap();
    let mut want = vec![Color::BLUE; ZONE_COUNT];
    want[3] = Color::GREEN;
    assert_eq!(s.state.borrow().effect, Effect::Custom { colors: want.clone() });
    // a second zone edits the existing Custom, not the frame
    *s.frame.lock().unwrap() = [Color::BLACK; ZONE_COUNT];
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
    *s.frame.lock().unwrap() = [Color::BLACK; ZONE_COUNT];
    s.proxy.set_all_color("ff0000").await.unwrap();
    s.proxy.set_zone_color(0, "00ff00").await.unwrap();
    let mut want = vec![Color::RED; ZONE_COUNT];
    want[0] = Color::GREEN;
    assert_eq!(s.state.borrow().effect, Effect::Custom { colors: want });
}
