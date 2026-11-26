//! The daemon's state and tasks, shared by the Linux service and the Windows app's in-process core.

use crate::input::run_input;
use crate::keys::{KeyHub, emit_keys};
use crate::persist::{load_profiles, saver, watcher};
use crate::service::{DaemonCore, DaemonService, watch_connection};
use masterkeys_core::LightingState;
use crate::profiles::Profiles;
use masterkeys_core::profile::ProfileStore;
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
        let (dirty_tx, dirty) = watch::channel(0u64);
        let profiles = Arc::new(Mutex::new(profiles));
        let (keys, want_reader) = KeyHub::new();
        let core = DaemonCore {
            state: Arc::new(state_tx),
            connected: connected_rx,
            frame: Arc::new(Mutex::new([Color::BLACK; ZONE_COUNT])),
            keys,
            profiles: profiles.clone(),
            dirty: Arc::new(dirty_tx),
            bindings: Arc::new(watch::channel(Default::default()).0),
            input: Arc::new(watch::channel(Default::default()).0),
            gamesense: Arc::new(watch::channel(Default::default()).0),
        };
        core.gamesense.send_modify(|g| g.enabled = core.profiles.lock().unwrap().gamesense_enabled());
        core.refresh_bindings();
        Daemon { core, store, profiles, want_reader, dirty, state, connected }
    }

    pub fn service(&self) -> DaemonService {
        DaemonService::new(self.core.clone())
    }

    /// Start the daemon's tasks on the current tokio runtime, serving clients on `conn`. With `device`
    /// (the Linux service) it drives the Apex 3 and serves GameSense; without (the Windows app's core,
    /// and tests) any keyboard counts as connected.
    pub fn spawn_tasks(self, conn: zbus::Connection, device: bool) {
        let Daemon { core, store, profiles, want_reader, dirty, state, connected } = self;
        let (key_tx, key_rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(run_input(core.clone(), conn.clone(), want_reader, key_tx));
        tokio::spawn(emit_keys(conn.clone(), core.keys.clone(), key_rx));
        tokio::spawn(watch_connection(conn.clone(), core.connected.clone()));
        tokio::spawn(saver(store, profiles, dirty));
        #[cfg(target_os = "linux")]
        if device {
            use crate::output::{self, Backoff, Shared};
            // GameSense colours over the profile; the sender goes to the GameSense task.
            let (overlay_tx, overlay) = watch::channel([None; ZONE_COUNT]);
            let shared = Shared { state, connected, frame: core.frame.clone(), overlay };
            tokio::spawn(output::run(crate::device::apex3_opener(), shared, Backoff::DEFAULT));
            tokio::spawn(crate::gamesense::run_gamesense(core.clone(), conn.clone(), overlay_tx));
            tokio::spawn(crate::keys::drop_vanished(conn.clone(), core.keys.clone()));
            tokio::spawn(watcher(core, conn));
            return;
        }
        {
            let _ = device;
            // No device to drive: any keyboard counts as connected, for as long as the core runs.
            drop(state);
            connected.send_replace(true);
            tokio::spawn(async move {
                let _connected = connected;
                std::future::pending::<()>().await
            });
        }
        tokio::spawn(watcher(core, conn));
    }
}
