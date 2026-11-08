//! The org.masterkeys.Daemon1 D-Bus interface.

use crate::render::Renderer;
use masterkeys_core::{LightingState, StateReport, parse_hex};
use std::sync::{Arc, Mutex};
use steelseries_gg::apex3::ZONE_COUNT;
use steelseries_gg::rgb::{Color, Effect};
use tokio::sync::watch;
use tracing::warn;
use zbus::object_server::SignalEmitter;
use zbus::{fdo, interface};

/// D-Bus errors, named `org.masterkeys.Daemon1.Error.*`.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.masterkeys.Daemon1.Error")]
pub enum ServiceError {
    #[zbus(error)]
    ZBus(zbus::Error),
    InvalidArgument(String),
    Unsupported(String),
}

pub struct DaemonService {
    state: watch::Sender<LightingState>,
    connected: watch::Receiver<bool>,
    frame: Arc<Mutex<[Color; ZONE_COUNT]>>,
}

impl DaemonService {
    pub fn new(
        state: watch::Sender<LightingState>,
        connected: watch::Receiver<bool>,
        frame: Arc<Mutex<[Color; ZONE_COUNT]>>,
    ) -> Self {
        Self { state, connected, frame }
    }

    fn report_json(&self) -> String {
        let report = StateReport { state: self.state.borrow().clone(), connected: *self.connected.borrow() };
        serde_json::to_string(&report).expect("state serializes")
    }

    /// Validate and publish a new state, then tell listeners.
    async fn apply(&self, candidate: LightingState, emitter: &SignalEmitter<'_>) -> Result<(), ServiceError> {
        if let Err(message) = candidate.validate() {
            return Err(match candidate.effect {
                Effect::Reactive { .. } => ServiceError::Unsupported(message),
                _ => ServiceError::InvalidArgument(message),
            });
        }
        self.state.send_replace(candidate);
        if let Err(e) = Self::state_changed(emitter, self.report_json()).await {
            warn!("could not emit StateChanged: {e}");
        }
        Ok(())
    }

    /// What the current state shows: rendered directly when it doesn't animate (the output loop may not
    /// have sent it yet), otherwise the frame last sent.
    fn shown_colors(&self) -> Vec<Color> {
        let mut renderer = Renderer::new();
        renderer.set_state(&self.state.borrow());
        if renderer.next_frame_in().is_none() {
            renderer.frame().to_vec()
        } else {
            self.frame.lock().unwrap().to_vec()
        }
    }

    fn with_effect(&self, effect: Effect) -> LightingState {
        LightingState { effect, ..self.state.borrow().clone() }
    }
}

fn invalid(message: String) -> ServiceError {
    ServiceError::InvalidArgument(message)
}

#[interface(name = "org.masterkeys.Daemon1")]
impl DaemonService {
    /// JSON `StateReport`: the lighting state plus whether the keyboard is connected.
    fn get_state(&self) -> String {
        self.report_json()
    }

    async fn set_all_color(
        &self,
        color: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), ServiceError> {
        let color = parse_hex(color).map_err(invalid)?;
        self.apply(self.with_effect(Effect::Static { color }), &emitter).await
    }

    /// Set one zone (0 leftmost .. 9 rightmost). Starts from the colours shown if not already per-zone.
    async fn set_zone_color(
        &self,
        zone: u8,
        color: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), ServiceError> {
        let zone = usize::from(zone);
        if zone >= ZONE_COUNT {
            return Err(invalid(format!("zone must be 0-{}, got {zone}", ZONE_COUNT - 1)));
        }
        let color = parse_hex(color).map_err(invalid)?;
        let mut colors = self.shown_colors();
        colors[zone] = color;
        self.apply(self.with_effect(Effect::Custom { colors }), &emitter).await
    }

    async fn set_brightness(
        &self,
        percent: u8,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), ServiceError> {
        let candidate = LightingState { brightness: percent, ..self.state.borrow().clone() };
        self.apply(candidate, &emitter).await
    }

    /// JSON `Effect`, e.g. `{"Spectrum":{"speed":0.5}}` or `"Off"`.
    async fn set_effect(
        &self,
        effect_json: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), ServiceError> {
        let effect: Effect =
            serde_json::from_str(effect_json).map_err(|e| invalid(format!("not a valid effect: {e}")))?;
        self.apply(self.with_effect(effect), &emitter).await
    }

    #[zbus(signal)]
    async fn state_changed(emitter: &SignalEmitter<'_>, state_json: String) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn device_connected(emitter: &SignalEmitter<'_>, connected: bool) -> zbus::Result<()>;
}

/// Emit `DeviceConnected` on every connect/disconnect transition, until the output loop goes away.
pub async fn watch_connection(conn: zbus::Connection, mut connected: watch::Receiver<bool>) -> fdo::Result<()> {
    let emitter = SignalEmitter::new(&conn, masterkeys_core::dbus::OBJECT_PATH)?;
    while connected.changed().await.is_ok() {
        let now = *connected.borrow_and_update();
        DaemonService::device_connected(&emitter, now).await?;
    }
    Ok(())
}
