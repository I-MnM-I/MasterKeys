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
    let (loaded, warnings) = store.load_all();
    let mut profiles = Profiles::from_loaded(loaded, store.load_settings());
    profiles.reserve(store.saved_names());
    (profiles, warnings)
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
            // Write the in-memory profile, which may hold edits not yet saved under the old name.
            StoreOp::Rename(old, new) => match snapshot.get(new) {
                Some(profile) => store.save(profile).and_then(|()| store.delete(old)),
                None => store.delete(old), // renamed again since; a later op writes it
            },
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

#[cfg(test)]
mod tests {
    use super::*;
    use masterkeys_core::LightingState;

    #[test]
    fn rename_right_after_an_edit_keeps_the_edit() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(dir.path());
        let (profiles, _) = load_profiles(&store);
        let profiles = Mutex::new(profiles);
        flush(&store, &profiles);
        let red = LightingState { brightness: 42, ..LightingState::default() };
        {
            let mut p = profiles.lock().unwrap();
            p.set_lighting(red.clone());
            p.rename("Default", "Renamed").unwrap();
        }
        flush(&store, &profiles);
        let (loaded, _) = store.load_all();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "Renamed");
        assert_eq!(loaded[0].lighting, red);
    }

    #[test]
    fn unreadable_profile_files_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(dir.path());
        std::fs::create_dir_all(dir.path().join("profiles")).unwrap();
        let broken = dir.path().join("profiles/broken.toml");
        std::fs::write(&broken, "this is = = not toml").unwrap();
        let (profiles, warnings) = load_profiles(&store);
        assert!(warnings.iter().any(|w| w.contains("broken.toml")));
        let profiles = Mutex::new(profiles);
        assert!(profiles.lock().unwrap().create("broken").is_err());
        assert!(profiles.lock().unwrap().rename("Default", "broken").is_err());
        flush(&store, &profiles);
        assert_eq!(std::fs::read_to_string(&broken).unwrap(), "this is = = not toml");
    }
}
