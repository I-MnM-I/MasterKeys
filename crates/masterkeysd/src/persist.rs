//! Keeping profiles on disk, and switching profile when linked programs start or exit.

use crate::profiles::{AutoSwitch, Profiles, StoreOp};
use crate::service::DaemonCore;
use masterkeys_core::process;
use masterkeys_core::profile::ProfileStore;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;
use tracing::{info, warn};

/// How long edits must pause before they are written.
const SAVE_DELAY: Duration = Duration::from_secs(1);
/// How often running programs are checked for auto-switch.
const WATCH_INTERVAL: Duration = Duration::from_secs(2);

/// Load profiles and settings; the strings are warnings about files that were skipped.
pub fn load_profiles(store: &ProfileStore) -> (Profiles, Vec<String>) {
    let (profiles, warnings) = store.load_all();
    (Profiles::from_loaded(profiles, store.load_settings()), warnings)
}

/// Write pending changes: once at start (first-run defaults), then whenever edits pause for
/// `SAVE_DELAY`, and a last time when the daemon's side of `dirty` goes away.
pub async fn saver(store: ProfileStore, profiles: Arc<Mutex<Profiles>>, mut dirty: watch::Receiver<u64>) {
    flush(&store, &profiles);
    'edits: while dirty.changed().await.is_ok() {
        loop {
            tokio::select! {
                changed = dirty.changed() => if changed.is_err() { break 'edits },
                _ = tokio::time::sleep(SAVE_DELAY) => break,
            }
        }
        flush(&store, &profiles);
    }
    flush(&store, &profiles);
}

/// Write every pending change now.
pub fn flush(store: &ProfileStore, profiles: &Mutex<Profiles>) {
    let (ops, snapshot) = {
        let mut p = profiles.lock().unwrap();
        (p.take_pending(), p.clone())
    };
    for op in ops {
        let result = match &op {
            StoreOp::Save(name) => match snapshot.get(name) {
                Some(profile) => store.save(profile),
                None => Ok(()), // renamed or deleted since; a later op covers it
            },
            StoreOp::Delete(name) => store.delete(name),
            StoreOp::Rename(old, new) => store.rename(old, new).or_else(|_| match snapshot.get(new) {
                // Not on disk yet (created and renamed between saves): just write it.
                Some(profile) => store.save(profile),
                None => Ok(()),
            }),
            StoreOp::Settings => store.save_settings(&snapshot.settings()),
        };
        if let Err(e) = result {
            warn!("could not save profiles ({op:?}): {e}");
        }
    }
}

/// Poll running programs and switch profile when linked ones start or exit.
pub async fn watcher(core: DaemonCore, conn: zbus::Connection) {
    let mut auto = AutoSwitch::default();
    let mut ticker = tokio::time::interval(WATCH_INTERVAL);
    loop {
        ticker.tick().await;
        let Ok(running) = tokio::task::spawn_blocking(process::running).await else { continue };
        let target = {
            let profiles = core.profiles.lock().unwrap();
            auto.update(&profiles, &running)
        };
        let Some(name) = target else { continue };
        if core.profiles.lock().unwrap().active().name == name {
            continue;
        }
        match core.activate(&name) {
            Ok(()) => {
                info!("auto-switch to profile {name:?}");
                core.announce(&conn, false).await;
            }
            Err(e) => warn!("auto-switch to {name:?} failed: {e}"),
        }
    }
}
