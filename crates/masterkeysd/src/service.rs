//! The org.masterkeys.Daemon1 D-Bus interface.

use crate::keys::{KeyHub, P2P_PEER};
use crate::profiles::{ProfileError, Profiles};
use masterkeys_core::dbus::OBJECT_PATH;
use masterkeys_core::profile::ProfileSummary;
use masterkeys_core::render::Renderer;
use masterkeys_core::{LightingState, StateReport, parse_hex};
use std::sync::{Arc, Mutex};
use steelseries_gg::apex3::ZONE_COUNT;
use steelseries_gg::rgb::{Color, Effect};
use tokio::sync::watch;
use tracing::warn;
use zbus::message::Header;
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
    /// Allowed in principle but against a rule, e.g. deleting the default profile.
    Refused(String),
}

impl From<ProfileError> for ServiceError {
    fn from(e: ProfileError) -> Self {
        match e {
            ProfileError::Invalid(m) => ServiceError::InvalidArgument(m),
            ProfileError::Refused(m) => ServiceError::Refused(m),
        }
    }
}

/// Everything the service and the background tasks share.
#[derive(Clone)]
pub struct DaemonCore {
    /// What the output loop shows; always the active profile's lighting.
    pub state: Arc<watch::Sender<LightingState>>,
    pub connected: watch::Receiver<bool>,
    pub frame: Arc<Mutex<[Color; ZONE_COUNT]>>,
    pub keys: KeyHub,
    pub profiles: Arc<Mutex<Profiles>>,
    /// Bumped on every change the saver should write.
    pub dirty: Arc<watch::Sender<u64>>,
}

impl DaemonCore {
    pub fn report_json(&self) -> String {
        let report = StateReport {
            state: self.state.borrow().clone(),
            connected: *self.connected.borrow(),
            profile: self.profiles.lock().unwrap().active().name.clone(),
        };
        serde_json::to_string(&report).expect("state serializes")
    }

    fn mark_dirty(&self) {
        self.dirty.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// Switch to a profile and show its lighting.
    pub fn activate(&self, name: &str) -> Result<(), ProfileError> {
        let lighting = {
            let mut profiles = self.profiles.lock().unwrap();
            profiles.activate(name)?;
            profiles.active().lighting.clone()
        };
        self.state.send_replace(lighting);
        self.mark_dirty();
        Ok(())
    }

    /// Tell listeners the state (and, if `profiles` is set, the profile list) changed.
    pub async fn announce(&self, conn: &zbus::Connection, profiles: bool) {
        let Ok(emitter) = SignalEmitter::new(conn, OBJECT_PATH) else { return };
        self.announce_with(&emitter, profiles).await;
    }

    async fn announce_with(&self, emitter: &SignalEmitter<'_>, profiles: bool) {
        if let Err(e) = DaemonService::state_changed(emitter, self.report_json()).await {
            warn!("could not emit StateChanged: {e}");
        }
        if profiles && let Err(e) = DaemonService::profiles_changed(emitter).await {
            warn!("could not emit ProfilesChanged: {e}");
        }
    }
}

pub struct DaemonService {
    core: DaemonCore,
}

impl DaemonService {
    pub fn new(core: DaemonCore) -> Self {
        Self { core }
    }

    /// Validate a new lighting state, store it in the active profile and show it.
    async fn apply(&self, candidate: LightingState, emitter: &SignalEmitter<'_>) -> Result<(), ServiceError> {
        if let Err(message) = candidate.validate() {
            return Err(match candidate.effect {
                Effect::Reactive { .. } => ServiceError::Unsupported(message),
                _ => ServiceError::InvalidArgument(message),
            });
        }
        self.core.profiles.lock().unwrap().set_lighting(candidate.clone());
        self.core.state.send_replace(candidate);
        self.core.mark_dirty();
        self.core.announce_with(emitter, false).await;
        Ok(())
    }

    /// Run a profile change, then save and tell listeners; the shown lighting follows the active profile.
    async fn change_profiles(
        &self,
        emitter: &SignalEmitter<'_>,
        change: impl FnOnce(&mut Profiles) -> Result<(), ProfileError>,
    ) -> Result<(), ServiceError> {
        let lighting = {
            let mut profiles = self.core.profiles.lock().unwrap();
            change(&mut profiles)?;
            profiles.active().lighting.clone()
        };
        self.core.state.send_if_modified(|s| {
            let changed = *s != lighting;
            *s = lighting;
            changed
        });
        self.core.mark_dirty();
        self.core.announce_with(emitter, true).await;
        Ok(())
    }

    /// What the current state shows: rendered directly when it doesn't animate (the output loop may not
    /// have sent it yet), otherwise the frame last sent.
    fn shown_colors(&self) -> Vec<Color> {
        let mut renderer = Renderer::new();
        renderer.set_state(&self.core.state.borrow());
        if renderer.next_frame_in().is_none() {
            renderer.frame().to_vec()
        } else {
            self.core.frame.lock().unwrap().to_vec()
        }
    }

    fn with_effect(&self, effect: Effect) -> LightingState {
        LightingState { effect, ..self.core.state.borrow().clone() }
    }
}

fn invalid(message: String) -> ServiceError {
    ServiceError::InvalidArgument(message)
}

// `spawn = false`: calls run one at a time, so read-modify-write setters (SetZoneColor) never race.
#[interface(name = "org.masterkeys.Daemon1", spawn = false)]
impl DaemonService {
    /// JSON `StateReport`: the lighting, whether the keyboard is connected, and the active profile.
    fn get_state(&self) -> String {
        self.core.report_json()
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
        let candidate = LightingState { brightness: percent, ..self.core.state.borrow().clone() };
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

    /// JSON list of `ProfileSummary`, sorted by name.
    fn list_profiles(&self) -> String {
        let profiles = self.core.profiles.lock().unwrap();
        let list: Vec<ProfileSummary> = profiles
            .list()
            .into_iter()
            .map(|p| ProfileSummary {
                name: p.name.clone(),
                executables: p.executables.clone(),
                is_default: p.name == profiles.default_name(),
            })
            .collect();
        serde_json::to_string(&list).expect("list serializes")
    }

    async fn activate_profile(
        &self,
        name: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), ServiceError> {
        self.change_profiles(&emitter, |p| p.activate(name)).await
    }

    /// A new profile copying the current lighting; not activated.
    async fn create_profile(
        &self,
        name: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), ServiceError> {
        self.change_profiles(&emitter, |p| p.create(name)).await
    }

    async fn delete_profile(
        &self,
        name: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), ServiceError> {
        self.change_profiles(&emitter, |p| p.delete(name)).await
    }

    async fn rename_profile(
        &self,
        old: &str,
        new: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), ServiceError> {
        self.change_profiles(&emitter, |p| p.rename(old, new)).await
    }

    async fn set_profile_executables(
        &self,
        name: &str,
        executables: Vec<String>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), ServiceError> {
        self.change_profiles(&emitter, |p| p.set_executables(name, executables)).await
    }

    async fn set_default_profile(
        &self,
        name: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), ServiceError> {
        self.change_profiles(&emitter, |p| p.set_default(name)).await
    }

    /// Start sending this caller `KeyEvent` signals.
    fn subscribe_keys(&self, #[zbus(header)] header: Header<'_>) {
        self.core.keys.subscribe(caller(&header));
    }

    fn unsubscribe_keys(&self, #[zbus(header)] header: Header<'_>) {
        self.core.keys.unsubscribe(&caller(&header));
    }

    #[zbus(signal)]
    async fn state_changed(emitter: &SignalEmitter<'_>, state_json: String) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn profiles_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn device_connected(emitter: &SignalEmitter<'_>, connected: bool) -> zbus::Result<()>;

    /// Sent only to subscribers (see `keys::emit_keys`).
    #[zbus(signal)]
    pub async fn key_event(emitter: &SignalEmitter<'_>, code: u16, pressed: bool) -> zbus::Result<()>;
}

fn caller(header: &Header<'_>) -> String {
    header.sender().map_or_else(|| P2P_PEER.to_string(), |s| s.to_string())
}

/// Emit `DeviceConnected` on every connect/disconnect transition, until the output loop goes away.
pub async fn watch_connection(conn: zbus::Connection, mut connected: watch::Receiver<bool>) -> fdo::Result<()> {
    let emitter = SignalEmitter::new(&conn, OBJECT_PATH)?;
    while connected.changed().await.is_ok() {
        let now = *connected.borrow_and_update();
        DaemonService::device_connected(&emitter, now).await?;
    }
    Ok(())
}
