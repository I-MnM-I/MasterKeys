//! masterkeysd: owns the SteelSeries Apex 3 and serves lighting control on the session bus.

use masterkeys_core::dbus::{BUS_NAME, OBJECT_PATH};
use masterkeys_core::profile::ProfileStore;
use masterkeysd::device::apex3_opener;
use masterkeysd::input::run_input;
use masterkeysd::keys::{KeyHub, drop_vanished, emit_keys};
use masterkeysd::output::{self, Backoff, Shared};
use masterkeysd::persist::{flush, load_profiles, saver, watcher};
use masterkeysd::service::{DaemonCore, DaemonService, watch_connection};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use steelseries_gg::apex3::ZONE_COUNT;
use steelseries_gg::rgb::Color;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::watch;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use zbus::fdo::{RequestNameFlags, RequestNameReply};

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .init();

    let Some(config_dir) = std::env::var_os("MASTERKEYS_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .or_else(ProfileStore::default_config_dir)
    else {
        error!("cannot find a config directory (HOME is not set)");
        return ExitCode::FAILURE;
    };
    let store = ProfileStore::new(&config_dir);
    let (profiles, warnings) = load_profiles(&store);
    for warning in warnings {
        warn!("profile {warning}");
    }
    info!("profile {:?} active (config in {})", profiles.active().name, config_dir.display());

    let (state_tx, state_rx) = watch::channel(profiles.active().lighting.clone());
    let (connected_tx, connected_rx) = watch::channel(false);
    let frame = Arc::new(Mutex::new([Color::BLACK; ZONE_COUNT]));
    let (dirty_tx, dirty_rx) = watch::channel(0u64);
    let profiles = Arc::new(Mutex::new(profiles));

    let (keys, want_reader) = KeyHub::new();
    let core = DaemonCore {
        state: Arc::new(state_tx),
        connected: connected_rx.clone(),
        frame: frame.clone(),
        keys: keys.clone(),
        profiles: profiles.clone(),
        dirty: Arc::new(dirty_tx),
        bindings: Arc::new(watch::channel(Default::default()).0),
        input: Arc::new(watch::channel(Default::default()).0),
    };
    core.refresh_bindings();
    let service = DaemonService::new(core.clone());
    let conn = match zbus::connection::Builder::session().serve_at(OBJECT_PATH, service).build().await {
        Ok(conn) => conn,
        Err(e) => {
            error!("cannot start D-Bus service: {e}");
            return ExitCode::FAILURE;
        }
    };
    // Claim the name without queueing, so a second instance fails instead of waiting in line.
    match conn.request_name_with_flags(BUS_NAME, RequestNameFlags::DO_NOT_QUEUE).await {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {}
        Ok(_) | Err(zbus::Error::NameTaken) => {
            eprintln!("masterkeysd is already running");
            return ExitCode::FAILURE;
        }
        Err(e) => {
            error!("cannot claim {BUS_NAME}: {e}");
            return ExitCode::FAILURE;
        }
    }
    info!("serving {BUS_NAME} at {OBJECT_PATH}");

    // GameSense colours over the profile; the sender goes to the GameSense task.
    let (overlay_tx, overlay_rx) = watch::channel([None; ZONE_COUNT]);
    let shared = Shared { state: state_rx, connected: connected_tx, frame, overlay: overlay_rx };
    tokio::spawn(output::run(apex3_opener(), shared, Backoff::DEFAULT));
    let _overlay_tx = overlay_tx; // wired to GameSense in the next task
    let (key_tx, key_rx) = tokio::sync::mpsc::channel(256);
    tokio::spawn(run_input(core.clone(), conn.clone(), want_reader, key_tx));
    tokio::spawn(emit_keys(conn.clone(), keys.clone(), key_rx));
    tokio::spawn(drop_vanished(conn.clone(), keys));
    tokio::spawn(watch_connection(conn.clone(), connected_rx));
    tokio::spawn(saver(store.clone(), profiles.clone(), dirty_rx));
    tokio::spawn(watcher(core, conn.clone()));

    let (mut term, mut int) = match (signal(SignalKind::terminate()), signal(SignalKind::interrupt())) {
        (Ok(t), Ok(i)) => (t, i),
        _ => {
            error!("cannot install signal handlers");
            return ExitCode::FAILURE;
        }
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
    // Save any edits still waiting out the save delay. Exit without touching the keyboard, so it keeps
    // showing the last frame.
    flush(&store, &profiles);
    info!("shutting down");
    ExitCode::SUCCESS
}
