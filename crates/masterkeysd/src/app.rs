//! The daemon's state and tasks, shared by the Linux service and the Windows app's in-process core.

use crate::input::run_input;
use crate::keys::{KeyHub, emit_keys};
use crate::persist::{load_profiles, saver, watcher};
use crate::service::{DaemonCore, DaemonService, watch_connection};
use masterkeys_core::LightingState;
use crate::profiles::Profiles;
use masterkeys_core::profile::ProfileStore;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use steelseries_gg::apex3::ZONE_COUNT;
use steelseries_gg::rgb::Color;
use tokio::sync::watch;
use tracing::{info, warn};

/// Everything the daemon owns, loaded but not yet running.
pub struct Daemon {
    pub core: DaemonCore,
    pub store: ProfileStore,
    pub profiles: Arc<Mutex<Profiles>>,
    want_reader: watch::Receiver<bool>,
    dirty: watch::Receiver<u64>,
    state: watch::Receiver<LightingState>,
    connected: watch::Sender<bool>,
    devices: watch::Sender<BTreeSet<String>>,
}

impl Daemon {
    /// Load the profiles in `config_dir` (logging any that could not be read).
    pub fn load(config_dir: &Path) -> Daemon {
        let store = ProfileStore::new(config_dir);
        let (profiles, warnings) = load_profiles(&store);
        for warning in warnings {
            warn!("profile {warning}");
        }
        info!("profile {:?} active (config in {})", profiles.active().name, config_dir.display());

        let (state_tx, state) = watch::channel(profiles.active().lighting.clone());
        let (connected, connected_rx) = watch::channel(false);
        let (devices, devices_rx) = watch::channel(BTreeSet::new());
        let (dirty_tx, dirty) = watch::channel(0u64);
        let profiles = Arc::new(Mutex::new(profiles));
        let (keys, want_reader) = KeyHub::new();
        let core = DaemonCore {
            state: Arc::new(state_tx),
            connected: connected_rx,
            devices: devices_rx,
            frame: Arc::new(Mutex::new(vec![Color::BLACK; ZONE_COUNT])),
            keys,
            profiles: profiles.clone(),
            dirty: Arc::new(dirty_tx),
            bindings: Arc::new(watch::channel(Default::default()).0),
            input: Arc::new(watch::channel(Default::default()).0),
            gamesense: Arc::new(watch::channel(Default::default()).0),
        };
        core.gamesense.send_modify(|g| g.enabled = core.profiles.lock().unwrap().gamesense_enabled());
        core.refresh_bindings();
        Daemon { core, store, profiles, want_reader, dirty, state, connected, devices }
    }

    /// The sender the output loops report connected devices on; tests use it to pretend devices are there.
    pub fn devices_sender(&self) -> watch::Sender<BTreeSet<String>> {
        self.devices.clone()
    }

    pub fn service(&self) -> DaemonService {
        DaemonService::new(self.core.clone())
    }

    /// Start the daemon's tasks on the current tokio runtime, serving clients on `conn`. With `device`
    /// it drives every supported lighting device (and, on Linux, serves GameSense); without (tests) no
    /// hardware is touched. Remapping works with any keyboard, so on Windows, and without `device`, the
    /// core always counts as connected; on Linux with `device` it is connected while a lighting device is.
    pub fn spawn_tasks(self, conn: zbus::Connection, device: bool) {
        let Daemon { core, store, profiles, want_reader, dirty, state, connected, devices } = self;
        let (key_tx, key_rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(run_input(core.clone(), conn.clone(), want_reader, key_tx));
        tokio::spawn(emit_keys(conn.clone(), core.keys.clone(), key_rx));
        tokio::spawn(watch_connection(conn.clone(), core.connected.clone()));
        tokio::spawn(crate::service::watch_devices(conn.clone(), core.clone()));
        tokio::spawn(saver(store, profiles, dirty));
        if device {
            use crate::device::{apex3_opener, k745_opener};
            use crate::output::{self, Backoff, Shared};
            use masterkeys_core::devices::DeviceInfo;
            // GameSense colours over the Apex 3's profile; the sender goes to the GameSense task.
            let (overlay_tx, overlay) = watch::channel([None; ZONE_COUNT]);
            let apex = Shared {
                state: state.clone(),
                devices: devices.clone(),
                frame: Some(core.frame.clone()),
                overlay: Some(overlay),
            };
            tokio::spawn(output::run(apex3_opener(), DeviceInfo::apex3(), apex, Backoff::SCAN));
            let k745 = Shared { state, devices: devices.clone(), frame: None, overlay: None };
            tokio::spawn(output::run(k745_opener(), DeviceInfo::k745pro(), k745, Backoff::SCAN));
            #[cfg(target_os = "linux")]
            {
                tokio::spawn(crate::gamesense::run_gamesense(core.clone(), conn.clone(), overlay_tx));
                tokio::spawn(crate::keys::drop_vanished(conn.clone(), core.keys.clone()));
                // Connected while any lighting device is.
                let mut seen = core.devices.clone();
                tokio::spawn(async move {
                    loop {
                        let any = !seen.borrow_and_update().is_empty();
                        connected.send_replace(any);
                        if seen.changed().await.is_err() {
                            break;
                        }
                    }
                });
                tokio::spawn(watcher(core, conn));
                return;
            }
            #[cfg(not(target_os = "linux"))]
            drop(overlay_tx);
        } else {
            drop(state);
        }
        // Any keyboard counts as connected, for as long as the core runs.
        connected.send_replace(true);
        tokio::spawn(async move {
            let _keep = (connected, devices);
            std::future::pending::<()>().await
        });
        tokio::spawn(watcher(core, conn));
    }
}
